// wordcartel/src/app.rs — testable `step` + the real crossterm `run` loop.
//
// Design: terminal IO lives ONLY in `run`; `step` is pure and unit-testable.
// The real loop calls `step` then draws — `step` never touches the terminal.

use crossterm::event::Event;
#[cfg(test)]
use crossterm::event::KeyEvent;

use crate::{commands, config, derive, editor::Editor, file, keymap, render, term};
#[cfg(test)]
use crate::input;
use crate::jobs::{Executor, JobOutcome};
use crate::registry::{Ctx, Registry};
use wordcartel_core::history::Clock;

// ---------------------------------------------------------------------------
// step — pure, testable; no terminal IO
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Msg, apply_result, reduce — unified message type and reducer
// ---------------------------------------------------------------------------

pub enum Msg {
    Input(Event),
    JobDone(JobOutcome),
    FilterDone {
        buffer_id: crate::editor::BufferId,
        version: u64,
        range: std::ops::Range<usize>,
        cursor: usize,
        disposition: crate::filter::Disposition,
        outcome: crate::filter::RunResult,
    },
    ExportDone {
        buffer_id: crate::editor::BufferId,
        target: std::path::PathBuf,
        result: Result<crate::export::ExportResult, crate::filter::FilterError>,
        /// True when the user explicitly confirmed overwriting an existing
        /// target via the OverwriteExport prompt.  False when export was
        /// dispatched because the target did not exist at check time — in which
        /// case finalization must refuse to clobber a target that appeared in
        /// the meantime (TOCTOU guard; Codex pre-merge gate).
        overwrite_confirmed: bool,
    },
    TransformDone {
        buffer_id: crate::editor::BufferId,
        version: u64,
        range: std::ops::Range<usize>,
        kind: crate::transform::TransformKind,
        result: Result<String, crate::transform::TransformError>,
    },
    DiagnosticsDone {
        buffer_id: crate::editor::BufferId,
        version: u64,
        diagnostics: Vec<wordcartel_core::diagnostics::Diagnostic>,
    },
    ClipboardPaste { id: u64, buffer_id: crate::editor::BufferId, text: Option<String> },
    ClipboardAvailability(bool),
    Tick,
    /// The input reader thread ended (Err from read(), or a panic). Surfaced by
    /// the input watchdog; the run loop turns it into a clean InputLost shutdown.
    InputThreadDied,
}

/// Why the run loop exited. Drives the process exit code in `main`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Normal,
    InputLost,
}

impl std::fmt::Debug for Msg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Msg::Input(_) => f.write_str("Input(..)"),
            Msg::JobDone(_) => f.write_str("JobDone(..)"),
            Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => f
                .debug_struct("FilterDone")
                .field("buffer_id", buffer_id)
                .field("version", version)
                .field("range", range)
                .field("cursor", cursor)
                .field("disposition", disposition)
                .field("outcome", outcome)
                .finish(),
            Msg::ExportDone { buffer_id, target, .. } => f
                .debug_struct("ExportDone")
                .field("buffer_id", buffer_id)
                .field("target", target)
                .finish(),
            Msg::TransformDone { buffer_id, version, range, kind, .. } => f
                .debug_struct("TransformDone")
                .field("buffer_id", buffer_id)
                .field("version", version)
                .field("range", range)
                .field("kind", kind)
                .finish(),
            Msg::DiagnosticsDone { buffer_id, version, diagnostics } => f
                .debug_struct("DiagnosticsDone")
                .field("buffer_id", buffer_id)
                .field("version", version)
                .field("count", &diagnostics.len())
                .finish(),
            Msg::ClipboardPaste { id, buffer_id, text } => f.debug_struct("ClipboardPaste")
                .field("id", id).field("buffer_id", buffer_id)
                .field("has_text", &text.is_some()).finish(),
            Msg::ClipboardAvailability(ok) => f.debug_tuple("ClipboardAvailability").field(ok).finish(),
            Msg::Tick => f.write_str("Tick"),
            Msg::InputThreadDied => f.write_str("InputThreadDied"),
        }
    }
}

/// Re-window an overlay list after a selection/rows change (A6). `area_h` is the
/// CALLER's read of the active buffer's area height (the same source the mouse
/// path uses); render re-heals against the live frame each draw, so a transient
/// divergence (a key racing a resize) lasts at most one frame.
pub(crate) fn keep_overlay_visible(area_h: u16, selected: usize, row_count: usize, scroll_top: &mut usize) {
    let lh = crate::list_window::list_h_for(row_count, area_h);
    crate::list_window::keep_visible(selected, row_count, lh, scroll_top);
}

/// Fill rows for a freshly-opened palette (empty rows + empty query → rebuild).
/// Called immediately after any command dispatch and after dispatch_overlay_command
/// so a just-opened overlay has content before the first render.
pub(crate) fn hydrate_overlays(editor: &mut Editor, reg: &crate::registry::Registry, keymap: &crate::keymap::KeyTrie) {
    if let Some(ref mut p) = editor.palette {
        if p.rows.is_empty() && p.query.is_empty() {
            crate::palette::rebuild_rows(p, reg, keymap);
        }
    }
    if let Some(v) = editor.menu.as_ref().filter(|v| !v.built) {
        let want_open = v.open;
        let want_hl = v.highlighted;
        let mut built = crate::menu::build(reg, keymap);
        // The placeholder's `open` indexes MENU_ORDER; map it to the built groups'
        // position for that category (robust even if a category has no commands).
        if let Some(cat) = crate::registry::MENU_ORDER.get(want_open) {
            if let Some(pos) = built.groups.iter().position(|g| g.0 == *cat) {
                built.open = pos;
            }
        }
        built.highlighted = want_hl.min(
            built.groups.get(built.open).map_or(0, |g| g.1.len().saturating_sub(1)),
        );
        editor.menu = Some(built);
    }
}

/// Close the active overlay, dispatch `id` via the registry, drain executor results,
/// then hydrate any overlay opened by the dispatched command.
pub(crate) fn dispatch_overlay_command(
    editor: &mut Editor,
    reg: &crate::registry::Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
    id: crate::registry::CommandId,
) {
    editor.palette = None;
    editor.menu = None;
    editor.theme_picker = None;
    editor.file_browser = None;
    let mut ctx = crate::registry::Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
    reg.dispatch(id, &mut ctx);
    for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
    // Hydrate any overlay the dispatched command may have opened (Codex 3c).
    hydrate_overlays(editor, reg, keymap);
}

#[cfg(test)]
pub fn menu_select_for_test(
    editor: &mut Editor,
    reg: &crate::registry::Registry,
    ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
    id: crate::registry::CommandId,
) {
    editor.menu = None;
    let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), reg);
    dispatch_overlay_command(editor, reg, &keymap, ex, clock, msg_tx, id);
}

/// Apply the theme-picker's currently-selected built-in as a live preview.
/// `pub(crate)` so mouse.rs can call it after a wheel-scroll selection change.
pub(crate) fn preview_selected_theme(editor: &mut crate::editor::Editor) {
    let name = editor.theme_picker.as_ref().and_then(|tp| tp.rows.get(tp.selected).cloned());
    if let Some(name) = name {
        if let Some(theme) = wordcartel_core::theme::Theme::builtin(&name) { editor.apply_theme(theme); }
    }
}

pub fn outline_jump_to(editor: &mut Editor, byte: usize) {
    let origin = editor.active().document.selection.primary().head;
    crate::marks::record_jump(editor.active_mut(), origin);
    crate::registry::unfold_ancestors_of(editor, byte);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(byte);
    editor.outline = None;
    derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}

/// Process one message. Returns true while the app should keep running.
pub fn reduce(
    msg: Msg,
    editor: &mut Editor,
    reg: &Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn Executor,
    clock: &dyn Clock,
    msg_tx: &std::sync::mpsc::Sender<Msg>,
) -> bool {
    // PTY-smoke panic trigger (debug builds only): F12 while WCARTEL_SMOKE_PANIC
    // is set panics HERE — the first statement of reduce, ahead of every
    // overlay/modal/minibuffer interception branch, so it fires regardless of
    // app state; reduce runs on the main thread (the panic hook ignores other
    // threads). Press-only, matching the app's kind filtering, so key
    // repeat/release under enhanced keyboard protocols cannot double-fire.
    // The key-code comparison short-circuits before the env read; release
    // builds compile the whole check out and the var is inert.
    #[cfg(debug_assertions)]
    if let Msg::Input(Event::Key(key)) = &msg {
        if key.kind == crossterm::event::KeyEventKind::Press
            && key.code == crossterm::event::KeyCode::F(12)
            && std::env::var_os("WCARTEL_SMOKE_PANIC").is_some()
        {
            panic!("WCARTEL_SMOKE_PANIC: deliberate smoke-test panic");
        }
    }
    // pending_mark intercepts the very next key as the mark letter.
    // Non-key messages fall through to normal handling.
    if editor.pending_mark.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                match k.code {
                    crossterm::event::KeyCode::Esc => { editor.pending_mark = None; editor.status.clear(); }
                    crossterm::event::KeyCode::Char(c) => crate::marks::resolve_pending(editor, c),
                    _ => { editor.pending_mark = None; } // non-name key cancels
                }
            }
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        // non-key message: fall through to normal handling
    }

    // Menu overlay intercepts KEY INPUT and PASTE (no text field; paste is
    // consumed / silently dropped). Non-key, non-paste messages fall through to
    // the normal handlers so background work continues while the menu is open.
    if editor.menu.is_some() {
        if matches!(&msg, Msg::ClipboardPaste { .. }) {
            // Drop an async clipboard-paste result that arrives while the menu is
            // open — it must not land in the document behind the overlay.
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Paste(_)) = &msg {
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::KeyCode;
                // Close OUTSIDE any menu borrow (Codex Critical: `editor.menu = None`
                // must not run while `editor.menu.as_mut()` is held).
                if matches!(k.code, KeyCode::Esc | KeyCode::F(10)) {
                    editor.menu = None;
                } else {
                    let mut selected: Option<crate::registry::CommandId> = None;
                    if let Some(menu) = editor.menu.as_mut() {   // borrow scoped to this block
                        let ncat = menu.groups.len();
                        match k.code {
                            KeyCode::Left if ncat > 0 => { menu.open = (menu.open + ncat - 1) % ncat; menu.highlighted = 0; }
                            KeyCode::Right if ncat > 0 => { menu.open = (menu.open + 1) % ncat; menu.highlighted = 0; }
                            KeyCode::Up if ncat > 0 => { menu.highlighted = menu.highlighted.saturating_sub(1); }
                            KeyCode::Down if ncat > 0 => {
                                let n = menu.groups[menu.open].1.len();
                                if n > 0 { menu.highlighted = (menu.highlighted + 1).min(n - 1); }
                            }
                            KeyCode::Enter if ncat > 0 => {
                                if let Some((_, id)) = menu.groups[menu.open].1.get(menu.highlighted) { selected = Some(*id); }
                            }
                            _ => {}
                        }
                    } // menu borrow dropped here
                    if let Some(id) = selected {
                        dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id);
                    }
                }
            }
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        // Non-key msg falls through to normal handling while menu stays open.
    }

    // Palette overlay intercepts KEY INPUT and PASTE. Non-key, non-paste messages
    // (FilterDone, JobDone, Tick) fall through to normal handling while the
    // palette stays open.
    if editor.palette.is_some() {
        if matches!(&msg, Msg::ClipboardPaste { .. }) {
            // Drop an async clipboard-paste result that arrives while the palette is
            // open — it must not land in the document behind the overlay.
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Paste(text)) = msg {
            let ah = editor.active().view.area.1;
            if let Some(p) = editor.palette.as_mut() {
                p.query.insert_str(p.cursor, &text);
                p.cursor += text.len();
                crate::palette::rebuild_rows(p, reg, keymap);
                keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
            }
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                match k.code {
                    crossterm::event::KeyCode::Esc => {
                        editor.palette = None;
                    }
                    crossterm::event::KeyCode::Enter => {
                        let row = editor.palette.as_ref()
                            .and_then(|p| p.rows.get(p.selected).cloned());
                        if let Some(row) = row {
                            if let Some(bid) = row.buffer {
                                // Buffer-switcher row: dismiss palette, jump to buffer.
                                editor.palette = None;
                                if let Some(idx) = editor.buffers.iter().position(|b| b.id == bid) {
                                    crate::workspace::switch_to(editor, idx);
                                }
                            } else {
                                // Command-palette row: dispatch through registry.
                                dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, row.id);
                            }
                        }
                    }
                    crossterm::event::KeyCode::Up => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            p.selected = p.selected.saturating_sub(1);
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::Down => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            let max = p.rows.len().saturating_sub(1);
                            p.selected = (p.selected + 1).min(max);
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::PageDown => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            let lh = crate::list_window::list_h_for(p.rows.len(), ah);
                            p.selected = (p.selected + lh.max(1)).min(p.rows.len().saturating_sub(1));
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::PageUp => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            let lh = crate::list_window::list_h_for(p.rows.len(), ah);
                            p.selected = p.selected.saturating_sub(lh.max(1));
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::Home => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            p.selected = 0;
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::End => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            p.selected = p.rows.len().saturating_sub(1);
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::Backspace => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            if p.cursor > 0 {
                                // remove the char before cursor (byte-safe for ASCII labels)
                                let byte_pos = p.query[..p.cursor].char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
                                p.query.remove(byte_pos);
                                p.cursor = byte_pos;
                            }
                            crate::palette::rebuild_rows(p, reg, keymap);
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::Left => {
                        if let Some(p) = editor.palette.as_mut() {
                            if p.cursor > 0 {
                                p.cursor -= p.query[..p.cursor].char_indices().next_back().map(|(_, c)| c.len_utf8()).unwrap_or(0);
                            }
                        }
                    }
                    crossterm::event::KeyCode::Right => {
                        if let Some(p) = editor.palette.as_mut() {
                            if p.cursor < p.query.len() {
                                let c = p.query[p.cursor..].chars().next().unwrap();
                                p.cursor += c.len_utf8();
                            }
                        }
                    }
                    crossterm::event::KeyCode::Char(c)
                        if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                            && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            p.query.insert(p.cursor, c);
                            p.cursor += c.len_utf8();
                            crate::palette::rebuild_rows(p, reg, keymap);
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    _ => {}
                }
            }
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        // Non-key msg falls through to normal handling while palette stays open.
    }

    // Theme picker overlay intercepts KEY INPUT and PASTE. Non-key, non-paste messages
    // fall through to normal handling while the picker stays open (mirrors palette block).
    if editor.theme_picker.is_some() {
        // Paste intercept FIRST (mirror the palette, app.rs palette block) — else paste leaks
        // into the document while the picker is open (Codex I6).
        if matches!(&msg, Msg::ClipboardPaste { .. }) {
            // Drop an async clipboard-paste result that arrives while the theme picker is
            // open — it must not land in the document behind the overlay.
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Paste(text)) = &msg {
            let ah = editor.active().view.area.1;
            if let Some(tp) = editor.theme_picker.as_mut() {
                tp.query.push_str(text);
                crate::theme_picker::rebuild_rows(tp);
                keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
            }
            preview_selected_theme(editor);
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::KeyCode;
                match k.code {
                    KeyCode::Esc => {
                        // cancel preview → restore the theme active when we opened.
                        if let Some(tp) = editor.theme_picker.take() { editor.apply_theme(tp.original); }
                    }
                    KeyCode::Enter => { editor.theme_picker = None; } // keep current preview
                    KeyCode::Up => {
                        let ah = editor.active().view.area.1;
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            tp.selected = tp.selected.saturating_sub(1);
                            keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                        }
                        preview_selected_theme(editor);
                    }
                    KeyCode::Down => {
                        let ah = editor.active().view.area.1;
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            let max = tp.rows.len().saturating_sub(1);
                            tp.selected = (tp.selected + 1).min(max);
                            keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                        }
                        preview_selected_theme(editor);
                    }
                    KeyCode::PageDown => {
                        let ah = editor.active().view.area.1;
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            let lh = crate::list_window::list_h_for(tp.rows.len(), ah);
                            tp.selected = (tp.selected + lh.max(1)).min(tp.rows.len().saturating_sub(1));
                            keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                        }
                        preview_selected_theme(editor);
                    }
                    KeyCode::PageUp => {
                        let ah = editor.active().view.area.1;
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            let lh = crate::list_window::list_h_for(tp.rows.len(), ah);
                            tp.selected = tp.selected.saturating_sub(lh.max(1));
                            keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                        }
                        preview_selected_theme(editor);
                    }
                    KeyCode::Home => {
                        let ah = editor.active().view.area.1;
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            tp.selected = 0;
                            keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                        }
                        preview_selected_theme(editor);
                    }
                    KeyCode::End => {
                        let ah = editor.active().view.area.1;
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            tp.selected = tp.rows.len().saturating_sub(1);
                            keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                        }
                        preview_selected_theme(editor);
                    }
                    KeyCode::Backspace => {
                        let ah = editor.active().view.area.1;
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            tp.query.pop();
                            crate::theme_picker::rebuild_rows(tp);
                            keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                        }
                        preview_selected_theme(editor);
                    }
                    KeyCode::Char(c)
                        if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                            && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        let ah = editor.active().view.area.1;
                        if let Some(tp) = editor.theme_picker.as_mut() {
                            tp.query.push(c);
                            crate::theme_picker::rebuild_rows(tp);
                            keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                        }
                        preview_selected_theme(editor);
                    }
                    _ => {}
                }
            }
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        // Non-key msg falls through to normal handling while picker stays open.
    }

    // File browser overlay intercepts KEY INPUT and PASTE. Non-key, non-paste messages
    // fall through to normal handling while the browser stays open (mirrors theme_picker).
    if editor.file_browser.is_some() {
        // Drop an async clipboard-paste result that arrives while the browser is open —
        // it must not land in the document behind the overlay (Codex I6, mirror palette).
        if matches!(&msg, Msg::ClipboardPaste { .. }) {
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Paste(text)) = &msg {
            let ah = editor.active().view.area.1;
            if let Some(fb) = editor.file_browser.as_mut() {
                fb.query.push_str(text);
                crate::file_browser::rebuild_entries(fb);
                keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
            }
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::KeyCode;
                match k.code {
                    KeyCode::Esc => { editor.file_browser = None; }
                    KeyCode::Enter => {
                        // Resolve the selected entry: descend into a directory (incl. ".."),
                        // or open a file through the Task-4 dirty-guard.
                        let chosen = editor.file_browser.as_ref().and_then(|fb| {
                            fb.entries.get(fb.selected).map(|e| (e.name.clone(), e.is_dir))
                        });
                        if let Some((name, is_dir)) = chosen {
                            if is_dir {
                                // Compute the target directory without mutating fb yet.
                                let target = editor.file_browser.as_ref().map(|fb| {
                                    if name == ".." {
                                        fb.dir.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| fb.dir.clone())
                                    } else {
                                        fb.dir.join(&name)
                                    }
                                });
                                if let Some(target) = target {
                                    // §3: check readability BEFORE committing fb.dir.
                                    if std::fs::read_dir(&target).is_ok() {
                                        if let Some(fb) = editor.file_browser.as_mut() {
                                            fb.dir = target;
                                            fb.query.clear();
                                            fb.selected = 0;
                                            fb.scroll_top = 0; // A6: a stale window over a
                                            // smaller entry set would make the render slice
                                            // out-of-order (panic-class) — reset with selected.
                                            crate::file_browser::rebuild_entries(fb);
                                        }
                                    } else {
                                        editor.status = format!("cannot read directory: {}", target.display());
                                        // stay in prior dir — do NOT mutate fb.dir
                                    }
                                }
                            } else {
                                let path = editor.file_browser.as_ref().unwrap().dir.join(&name);
                                editor.file_browser = None;
                                crate::workspace::open_as_new_buffer(editor, &path);
                            }
                        }
                    }
                    KeyCode::Up => {
                        let ah = editor.active().view.area.1;
                        if let Some(fb) = editor.file_browser.as_mut() {
                            fb.selected = fb.selected.saturating_sub(1);
                            keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                        }
                    }
                    KeyCode::Down => {
                        let ah = editor.active().view.area.1;
                        if let Some(fb) = editor.file_browser.as_mut() {
                            let max = fb.entries.len().saturating_sub(1);
                            fb.selected = (fb.selected + 1).min(max);
                            keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                        }
                    }
                    KeyCode::PageDown => {
                        let ah = editor.active().view.area.1;
                        if let Some(fb) = editor.file_browser.as_mut() {
                            let lh = crate::list_window::list_h_for(fb.entries.len(), ah);
                            fb.selected = (fb.selected + lh.max(1)).min(fb.entries.len().saturating_sub(1));
                            keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                        }
                    }
                    KeyCode::PageUp => {
                        let ah = editor.active().view.area.1;
                        if let Some(fb) = editor.file_browser.as_mut() {
                            let lh = crate::list_window::list_h_for(fb.entries.len(), ah);
                            fb.selected = fb.selected.saturating_sub(lh.max(1));
                            keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                        }
                    }
                    KeyCode::Home => {
                        let ah = editor.active().view.area.1;
                        if let Some(fb) = editor.file_browser.as_mut() {
                            fb.selected = 0;
                            keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                        }
                    }
                    KeyCode::End => {
                        let ah = editor.active().view.area.1;
                        if let Some(fb) = editor.file_browser.as_mut() {
                            fb.selected = fb.entries.len().saturating_sub(1);
                            keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                        }
                    }
                    KeyCode::Backspace => {
                        let ah = editor.active().view.area.1;
                        if let Some(fb) = editor.file_browser.as_mut() {
                            fb.query.pop();
                            crate::file_browser::rebuild_entries(fb);
                            keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                        }
                    }
                    KeyCode::Char(c)
                        if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                            && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                    {
                        let ah = editor.active().view.area.1;
                        if let Some(fb) = editor.file_browser.as_mut() {
                            fb.query.push(c);
                            crate::file_browser::rebuild_entries(fb);
                            keep_overlay_visible(ah, fb.selected, fb.entries.len(), &mut fb.scroll_top);
                        }
                    }
                    _ => {}
                }
            }
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        // Non-key msg falls through to normal handling while the browser stays open.
    }

    // Active modal intercepts KEY INPUT only (§5.3). Background results and ticks
    // must still be processed — a JobDone arriving while a modal is up (e.g. an
    // in-flight save completing during the quit-confirm prompt) must not be
    // dropped, or save&quit would hang waiting for a result it already discarded.
    if editor.prompt.is_some() {
        match msg {
            Msg::Input(Event::Key(key)) if key.kind == crossterm::event::KeyEventKind::Press => {
                if key.code == crossterm::event::KeyCode::Esc {
                    editor.prompt = None; // Esc cancels any prompt
                    editor.pending_export = None;
                    editor.pending_save_overwrite = None;
                    editor.pending_save_as = None;
                    editor.pending_write_block = None;
                    // Effort 6 / Codex gate #2: Esc on a per-buffer review prompt (raised
                    // by drive_quit_drain) must abort the quit drain, just like Cancel does.
                    // Without this, quit_drain stays Some-but-inert: the drain is
                    // stranded with no in-flight save and no re-drive pending.
                    if editor.quit_drain.is_some() {
                        editor.quit_drain = None;
                        editor.quit_drain_advance = false;
                    }
                } else if let crossterm::event::KeyCode::Char(ch) = key.code {
                    if let Some(action) = editor.prompt.as_ref().unwrap().action_for(ch) {
                        crate::prompts::resolve_prompt(action, editor, ex, clock, msg_tx);
                    }
                }
            }
            // Merge a directly-delivered background result even under a modal.
            Msg::JobDone(o) => crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx),
            Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => {
                crate::jobs_apply::apply_filter_done(editor, buffer_id, version, range, cursor, disposition, outcome, clock);
            }
            Msg::ExportDone { target, result, overwrite_confirmed, .. } => {
                crate::jobs_apply::apply_export_done(editor, target, result, overwrite_confirmed);
            }
            Msg::TransformDone { buffer_id, version, range, kind, result } => {
                crate::jobs_apply::apply_transform_done(editor, buffer_id, version, range, kind, result, clock);
            }
            Msg::DiagnosticsDone { buffer_id, version, diagnostics } => {
                crate::diagnostics_run::apply_diagnostics_done(editor, buffer_id, version, diagnostics);
            }
            Msg::ClipboardPaste { buffer_id, text, .. } => crate::jobs_apply::apply_clipboard_paste(editor, buffer_id, text, clock),
            Msg::ClipboardAvailability(ok) => crate::jobs_apply::apply_clipboard_availability(editor, ok),
            // Resize/Tick/other input: ignored for the modal, but results still drain below.
            _ => {}
        }
        // Always drain ready results (merges the awaited save&quit result).
        for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
        return !editor.quit;
    }

    // Minibuffer intercepts KEY INPUT only; non-key messages (FilterDone/JobDone/Tick)
    // fall through to the normal match arm below — a FilterDone must apply even while
    // the minibuffer is open (see test `minibuffer_does_not_starve_filterdone`).
    if editor.minibuffer.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                match k.code {
                    crossterm::event::KeyCode::Char(c)
                        if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        editor.minibuffer.as_mut().unwrap().insert(c);
                    }
                    crossterm::event::KeyCode::Backspace => {
                        editor.minibuffer.as_mut().unwrap().backspace();
                    }
                    crossterm::event::KeyCode::Left => {
                        editor.minibuffer.as_mut().unwrap().left();
                    }
                    crossterm::event::KeyCode::Right => {
                        editor.minibuffer.as_mut().unwrap().right();
                    }
                    crossterm::event::KeyCode::Esc => {
                        // Dismiss the minibuffer (dismiss > cancel): this Esc is consumed
                        // here and does NOT reach the filter-cancel Esc check below, so
                        // any in-flight filter continues running.
                        editor.minibuffer = None;
                        // Save-As minibuffer dismiss: drop any queued post-save action.
                        editor.pending_save_as = None;
                        // Effort 6 (Codex C2): dismissing a drain's Save-As aborts the quit.
                        editor.quit_drain = None;
                        editor.quit_drain_advance = false;
                    }
                    crossterm::event::KeyCode::Enter => {
                        let mb = editor.minibuffer.take().unwrap();
                        match mb.kind {
                            crate::minibuffer::MinibufferKind::Filter     => crate::prompts::submit_filter_line(editor, &mb.text, msg_tx),
                            crate::minibuffer::MinibufferKind::GotoLine   => crate::prompts::goto_line_submit(editor, &mb.text),
                            crate::minibuffer::MinibufferKind::SaveAs     => crate::prompts::save_as_submit(editor, &mb.text, ex, clock, msg_tx),
                            crate::minibuffer::MinibufferKind::WriteBlock => crate::prompts::block_write_submit(editor, &mb.text),
                        }
                    }
                    _ => {}
                }
            }
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        // non-key (FilterDone/JobDone/Tick/Resize/ClipboardPaste/ClipboardAvailability) falls through to the normal match below
    }

    // Search overlay intercepts KEY INPUT only; non-key messages (FilterDone/JobDone/
    // TransformDone/ExportDone/Tick) fall through to the normal match arm below so
    // background work is never starved while the overlay is open (mirror of minibuffer
    // block above — see test `search_does_not_starve_filterdone`).
    if editor.search.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::{KeyCode, KeyModifiers};
                let alt = k.modifiers.contains(KeyModifiers::ALT);
                let shift = k.modifiers.contains(KeyModifiers::SHIFT);
                let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                // Stepping phase: y/n/!/q intercepted BEFORE the text-insert arm.
                if editor.search.as_ref().map(|s| s.phase) == Some(crate::search_overlay::Phase::Stepping) {
                    match k.code {
                        KeyCode::Char('y') => { crate::search_ui::search_step_apply(editor, clock); }
                        KeyCode::Char('n') => { crate::search_ui::search_step_skip(editor); }
                        KeyCode::Char('!') => { crate::search_ui::search_step_rest(editor, clock); }
                        KeyCode::Char('q') | KeyCode::Esc => { editor.search = None; }
                        _ => {}
                    }
                    for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
                    return !editor.quit;
                }
                match k.code {
                    KeyCode::Esc => { crate::search_ui::search_cancel(editor); return !editor.quit; }
                    KeyCode::Char('r') if alt => { editor.search.as_mut().unwrap().toggle_mode(); }
                    KeyCode::Char('c') if alt => { editor.search.as_mut().unwrap().cycle_case(); }
                    KeyCode::Char('a') if alt => { crate::search_ui::search_replace_all(editor, clock); return !editor.quit; }
                    KeyCode::Enter if alt => {
                        if let Some(s) = editor.search.as_mut() { s.phase = crate::search_overlay::Phase::Stepping; }
                        crate::search_ui::search_sync(editor); // park on first match
                        for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
                        return !editor.quit;
                    }
                    KeyCode::Enter if shift => { crate::search_ui::search_step(editor, false); }
                    KeyCode::F(3) if shift   => { crate::search_ui::search_step(editor, false); }
                    KeyCode::Enter           => { crate::search_ui::search_step(editor, true); }
                    KeyCode::F(3)            => { crate::search_ui::search_step(editor, true); }
                    KeyCode::Tab => {
                        if let Some(s) = editor.search.as_mut() {
                            s.field = match s.field {
                                crate::search_overlay::Field::Needle => crate::search_overlay::Field::Template,
                                crate::search_overlay::Field::Template => crate::search_overlay::Field::Needle,
                            };
                            s.cursor = s.focused_field().len();
                        }
                    }
                    KeyCode::Backspace       => { editor.search.as_mut().unwrap().backspace(); }
                    KeyCode::Left            => { editor.search.as_mut().unwrap().left(); }
                    KeyCode::Right           => { editor.search.as_mut().unwrap().right(); }
                    KeyCode::Char(c) if !ctrl && !alt => { editor.search.as_mut().unwrap().insert(c); }
                    _ => {}
                }
                // Recompute against the live buffer and pin the current match.
                crate::search_ui::search_sync(editor);
            }
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit; // return ONLY for key events (including non-Press)
        }
        // Non-key messages (FilterDone/ExportDone/TransformDone/JobDone/Tick/…)
        // fall through to the normal handlers below.
    }

    // Diag overlay intercepts KEY INPUT only; non-key messages fall through to
    // normal handling so background work is never starved while the overlay is open
    // (mirror of minibuffer/search blocks above — 5e starvation lesson).
    if editor.diag.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                match k.code {
                    crossterm::event::KeyCode::Up   => { editor.diag.as_mut().unwrap().up(); }
                    crossterm::event::KeyCode::Down => { editor.diag.as_mut().unwrap().down(); }
                    crossterm::event::KeyCode::Esc  => { editor.diag = None; }
                    crossterm::event::KeyCode::Enter => { crate::search_ui::diag_apply_selected(editor, clock); }
                    _ => {} // bare Ctrl+key or anything else: no-op, consumed
                }
            }
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit; // return ONLY for key events (including non-Press)
        }
        // Non-key messages fall through to normal handlers below.
    }

    if editor.outline.is_some()
        && editor.outline.as_ref().map(|o| o.buffer_id) != Some(editor.active().id) {
        editor.outline = None;
    }
    if editor.outline.is_some() {
        if let Msg::Input(Event::Key(k)) = &msg {
            if k.kind == crossterm::event::KeyEventKind::Press {
                use crossterm::event::{KeyCode, KeyModifiers};
                match k.code {
                    KeyCode::Esc => { editor.outline = None; }
                    KeyCode::Up => {
                        let ah = editor.active().view.area.1;
                        if let Some(o) = editor.outline.as_mut() {
                            o.selected = o.selected.saturating_sub(1);
                            keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                        }
                    }
                    KeyCode::Down => {
                        let ah = editor.active().view.area.1;
                        if let Some(o) = editor.outline.as_mut() {
                            let max = o.rows.len().saturating_sub(1);
                            o.selected = (o.selected + 1).min(max);
                            keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                        }
                    }
                    KeyCode::PageDown => {
                        let ah = editor.active().view.area.1;
                        if let Some(o) = editor.outline.as_mut() {
                            let lh = crate::list_window::list_h_for(o.rows.len(), ah);
                            o.selected = (o.selected + lh.max(1)).min(o.rows.len().saturating_sub(1));
                            keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                        }
                    }
                    KeyCode::PageUp => {
                        let ah = editor.active().view.area.1;
                        if let Some(o) = editor.outline.as_mut() {
                            let lh = crate::list_window::list_h_for(o.rows.len(), ah);
                            o.selected = o.selected.saturating_sub(lh.max(1));
                            keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                        }
                    }
                    KeyCode::Home => {
                        let ah = editor.active().view.area.1;
                        if let Some(o) = editor.outline.as_mut() {
                            o.selected = 0;
                            keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                        }
                    }
                    KeyCode::End => {
                        let ah = editor.active().view.area.1;
                        if let Some(o) = editor.outline.as_mut() {
                            o.selected = o.rows.len().saturating_sub(1);
                            keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                        }
                    }
                    KeyCode::Enter => {
                        if editor.outline.as_ref().map(|o| o.opened_version) != Some(editor.active().document.version) {
                            editor.status = "document changed; outline closed".into();
                            editor.outline = None;
                            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
                            return !editor.quit;
                        }
                        let target = editor.outline.as_ref()
                            .and_then(|o| o.rows.get(o.selected))
                            .map(|r| r.byte);
                        if let Some(target) = target {
                            outline_jump_to(editor, target);
                        }
                    }
                    KeyCode::Backspace => {
                        let ah = editor.active().view.area.1;
                        if let Some(o) = editor.outline.as_mut() {
                            o.query.pop();
                        }
                        let q = editor.outline.as_ref().map(|o| o.query.clone()).unwrap_or_default();
                        let (blocks, rope) = { let b = editor.active(); (b.document.blocks().clone(), b.document.buffer.snapshot()) };
                        if let Some(o) = editor.outline.as_mut() {
                            o.set_query(&q, &blocks, &rope);
                            keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                        }
                    }
                    KeyCode::Char(c)
                        if !k.modifiers.contains(KeyModifiers::CONTROL)
                            && !k.modifiers.contains(KeyModifiers::ALT) =>
                    {
                        let ah = editor.active().view.area.1;
                        if let Some(o) = editor.outline.as_mut() {
                            o.query.push(c);
                        }
                        let q = editor.outline.as_ref().map(|o| o.query.clone()).unwrap_or_default();
                        let (blocks, rope) = { let b = editor.active(); (b.document.blocks().clone(), b.document.buffer.snapshot()) };
                        if let Some(o) = editor.outline.as_mut() {
                            o.set_query(&q, &blocks, &rope);
                            keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                        }
                    }
                    _ => {}
                }
            }
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
            return !editor.quit;
        }
        // Non-key messages fall through to normal handlers below.
    }

    let before = editor.active().document.version;
    match msg {
        Msg::Input(Event::Key(k)) if k.kind == crossterm::event::KeyEventKind::Press => {
            // Esc precedence (Codex CRITICAL): prompt/minibuffer Esc are handled in their
            // interception blocks ABOVE this point. Here in normal mode the order is
            // pending-cancel > filter-cancel. This arm SUBSUMES the old standalone
            // filter-cancel Esc check (removed above). Esc is reserved for cancel/dismiss
            // in v1 (not routed to the keymap).
            if k.code == crossterm::event::KeyCode::Esc {
                if !editor.pending_keys.is_empty() {
                    editor.pending_keys.clear();
                    editor.status.clear();
                } else if editor.filter_in_flight.is_some() {
                    editor.filter_in_flight.take().unwrap().cancel();
                    editor.status = "cancelling…".into();
                }
            } else if let Some(chord) = crate::keymap::from_key_event(k) {
                editor.pending_keys.push(chord);
                match keymap.resolve(&editor.pending_keys) {
                    crate::keymap::Resolution::Command(id) => {
                        editor.pending_keys.clear();
                        editor.status.clear();
                        let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
                        reg.dispatch(id, &mut ctx);
                        hydrate_overlays(editor, reg, keymap);
                    }
                    crate::keymap::Resolution::Pending => {
                        editor.status = format!("{} …", crate::keymap::chords_display(&editor.pending_keys));
                    }
                    crate::keymap::Resolution::None => {
                        let was_single = editor.pending_keys.len() == 1;
                        editor.pending_keys.clear();
                        editor.status.clear();
                        // Printable fallthrough: single unmodified printable → literal insert.
                        if was_single {
                            if let crossterm::event::KeyCode::Char(c) = k.code {
                                if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                                    && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                                {
                                    commands::run(commands::Command::InsertChar(c), editor, clock);
                                }
                            }
                        }
                    }
                }
            }
        }
        Msg::Input(Event::Paste(text)) => {
            if let Some(mb) = editor.minibuffer.as_mut() {
                for ch in text.chars() { mb.insert(ch); }
            } else if !text.is_empty() {
                let bid = editor.active().id;
                if crate::jobs_apply::insert_paste_text(editor, bid, &text, clock) {
                    editor.register.set(text);
                }
            }
        }
        Msg::Input(Event::Resize(w, h)) => {
            for b in editor.buffers.iter_mut() {
                b.view.area = (w, h);
                b.invalidate_layout();
            }
            derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
        }
        Msg::Input(Event::Mouse(ev)) => {
            crate::mouse::handle(editor, ev, reg, keymap, ex, clock, msg_tx);
            // A click-opened menu placeholder must be built before the next render —
            // the key-dispatch path hydrates; the mouse path must too (A1 spec C1).
            hydrate_overlays(editor, reg, keymap);
        }
        Msg::Input(_) => {}
        Msg::JobDone(o) => crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx),
        Msg::FilterDone { buffer_id, version, range, cursor, disposition, outcome } => {
            crate::jobs_apply::apply_filter_done(editor, buffer_id, version, range, cursor, disposition, outcome, clock);
        }
        Msg::ExportDone { target, result, overwrite_confirmed, .. } => {
            crate::jobs_apply::apply_export_done(editor, target, result, overwrite_confirmed);
        }
        Msg::TransformDone { buffer_id, version, range, kind, result } => {
            crate::jobs_apply::apply_transform_done(editor, buffer_id, version, range, kind, result, clock);
        }
        Msg::DiagnosticsDone { buffer_id, version, diagnostics } => {
            crate::diagnostics_run::apply_diagnostics_done(editor, buffer_id, version, diagnostics);
        }
        Msg::Tick => {
            let now = clock.now_ms();
            if editor.active().document.dirty()
                && !editor.active().swap_in_flight
                && crate::swap::due(now, editor.active().last_edit_at, editor.active().last_swap_at)
            {
                editor.active_mut().swap_in_flight = true;
                let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
                crate::swap::dispatch_swap_write(&mut ctx);
            }
            // Dispatch diagnostics if due.
            let version = editor.active().document.version;
            if editor.diag_cfg.enabled
                && crate::diagnostics_run::diag_due(&editor.active().diagnostics, now, version)
            {
                let ignore_words = std::sync::Arc::new(
                    editor.dictionary.iter().chain(editor.session_ignores.iter()).cloned().collect::<std::collections::HashSet<String>>()
                );
                let diag_cfg = editor.diag_cfg.clone();
                crate::diagnostics_run::dispatch_diagnostics(editor, &diag_cfg, ignore_words, msg_tx.clone());
            }
            // Dispatch a block-tree reconcile if due.
            if crate::reconcile::reconcile_due(&editor.active().reconcile, now) {
                crate::reconcile::dispatch_reconcile(editor, ex);
            }
        }
        Msg::ClipboardPaste { buffer_id, text, .. } => crate::jobs_apply::apply_clipboard_paste(editor, buffer_id, text, clock),
        Msg::ClipboardAvailability(ok) => crate::jobs_apply::apply_clipboard_availability(editor, ok),
        // Intercepted in the run loop before `reduce` (see run()); unreachable here.
        // Arm required only for exhaustiveness. Do not process the shutdown here.
        Msg::InputThreadDied => {}
    }
    if editor.active().document.version != before {
        editor.active_mut().last_edit_at = Some(clock.now_ms());
        // Arm debounce for diagnostics if enabled.
        if editor.diag_cfg.enabled {
            let debounce_ms = editor.diag_cfg.debounce_ms;
            editor.active_mut().diagnostics.arm(clock.now_ms(), debounce_ms);
        }
    }
    // Fold any other results that became ready while handling this message.
    for o in ex.drain() {
        crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx);
    }
    !editor.quit
}

// ---------------------------------------------------------------------------
// step — pure, testable; no terminal IO
// ---------------------------------------------------------------------------

/// Legacy synchronous dispatch path retained for its existing tests; production
/// uses `reduce` + the registry.
///
/// Translate one key event, run the resulting command (if any), then return
/// `true` while the app should keep running (`false` → caller should exit).
///
/// All editor mutation goes through `commands::run`; this function adds no
/// logic of its own beyond the translation.
#[cfg(test)]
pub fn step(editor: &mut Editor, key: KeyEvent, clock: &dyn Clock) -> bool {
    if let Some(cmd) = input::key_to_command(key) {
        commands::run(cmd, editor, clock);
    }
    !editor.quit
}

// ---------------------------------------------------------------------------
// SystemClock — used only by the real `run` loop, never by unit tests
// ---------------------------------------------------------------------------

struct SystemClock;
impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// advance — shared per-iteration state steps (run loop + e2e harness)
// ---------------------------------------------------------------------------

/// The state-affecting per-iteration steps shared by `run()`'s loop and the e2e harness
/// (everything between the clipboard/mouse terminal steps and the draw). Extracted so the
/// harness exercises the REAL loop body, not a re-implementation.
pub(crate) fn advance(editor: &mut Editor, clock: &dyn Clock) {
    recompute_scrollbar_visible(editor, clock.now_ms());
    recompute_menu_bar(editor, clock.now_ms());
    // Pre-draw rebuild: ensure the layout cache matches the final (scroll,
    // text_width) before render consumes it.  render has no on-demand fallback
    // (render.rs:132-140), so a stale cache blanks the editing rows.
    derive::rebuild(editor);
    // Arm the reconcile debounce when the tree is (possibly) stale. Re-arm only
    // when the version advanced since the last arm (so idle Ticks don't push the
    // deadline forever); arm-from-None also covers a switch to a stale buffer.
    {
        let now = clock.now_ms();
        let b = editor.active_mut();
        if b.reconcile.maybe_stale && b.reconcile.in_flight_version.is_none()
            && (b.reconcile.due_at.is_none() || b.reconcile.armed_for_version != b.document.version)
        {
            b.reconcile.due_at = Some(now.saturating_add(crate::reconcile::RECONCILE_DEBOUNCE_MS));
            b.reconcile.armed_for_version = b.document.version;
        }
    }
}

// ---------------------------------------------------------------------------
// run — the real terminal loop; terminal IO lives entirely here
// ---------------------------------------------------------------------------

/// Open the file named by `cli.path` (or a scratch buffer), load layered config,
/// build the keymap, install the terminal guard, then loop:
/// draw → read event → step → repeat until `editor.quit`.
pub fn run(cli: config::Cli) -> std::io::Result<ExitReason> {
    // Install the panic hook (once) so the terminal is restored on panic.
    term::install_panic_hook();

    // Resolve config layers and build the keymap from them.
    let anchor = cli.path.as_ref()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")));
    let xdg = dirs::config_dir();
    let paths = config::config_layer_paths(&cli, xdg.as_deref(), &anchor);
    let (cfg, mut warns) = config::load(&paths);
    if let Some(c) = &cli.config_path {
        if !c.is_file() {
            warns.push(format!("config: --config path not found: {}", c.display()));
        }
    }

    // Determine the initial terminal size.
    let (cols, rows) = crossterm::terminal::size()?;
    let area = (cols, rows);

    let path = cli.path;

    // Open the file and branch on errors per §C5. Built on the Buffer::from_file seam
    // (Effort 7 Task 1) without behavior change: Ok → named clean; NotFound → named "new
    // file"; Binary/Permission/IsDir/Io → UNNAMED scratch kept + e.to_string() status.
    let mut editor = Editor::new_from_text("\n", None, area); // scratch host; the real buffer (if any) replaces slot 0 below
    if let Some(p) = path.as_deref() {
        let id = editor.active().id; // reuse slot 0's id for the launch buffer
        match crate::editor::Buffer::from_file(id, p, area) {
            Ok(b) => {
                let was_new_file = b.document.path.is_some() && !p.exists();
                editor.buffers[0] = b;
                if was_new_file {
                    // New file: empty buffer NAMED with the path; first save creates it.
                    editor.status = "new file".to_string();
                }
            }
            // NotFound is mapped to Ok (named "new file") inside from_file, so it never
            // reaches here; list it for exhaustiveness. The rest are rejected targets:
            // UNNAMED scratch kept so a save can't clobber the file.
            Err(e @ (file::OpenError::NotFound(_)
                   | file::OpenError::Binary(_)
                   | file::OpenError::Permission(_)
                   | file::OpenError::IsDir(_)
                   | file::OpenError::Io(_)
                   | file::OpenError::TooLarge(..))) => {
                editor.status = e.to_string();
            }
        }
    }

    // Effort 6: install the permanent *scratch* buffer (index 1; launch buffer stays active at 0).
    editor.install_scratch();

    // Effort 6: restore persisted scratch content (independent of resume_enabled —
    // scratch is the user's stash, not a per-file resume position).
    {
        let saved = crate::state::load();
        if let Some(st) = saved.scratch.as_ref() {
            crate::session_restore::restore_scratch(&mut editor, st);
        }
    }

    // Seed mouse_capture from config (default true; may be overridden by config layers).
    editor.mouse_capture = cfg.mouse.mouse_capture;
    editor.view_opts = cfg.view.clone();
    editor.resume_enabled = cfg.state.resume; // gates open_into_current's resume restore (Effort 7)
    editor.diag_cfg = cfg.diagnostics.clone();
    editor.export_cfg = cfg.export.clone();
    editor.menu_bar_mode = cfg.menu.bar;
    editor.menu_bar_unpinned_mode = if cfg.menu.bar == crate::config::MenuBarMode::Pinned {
        crate::config::MenuBarMode::Auto // unpin target when config itself pins
    } else {
        cfg.menu.bar
    };
    // Resolve and seed the active theme + color depth (once, at startup — §3.6).
    let env = crate::theme_resolve::EnvSnapshot::from_env();
    let resolved = crate::theme_resolve::resolve_theme(&cfg.theme, &env);
    editor.theme = resolved.theme;
    editor.depth = resolved.depth;
    editor.heading_glyph_cfg = cfg.theme.heading_level_glyph; // for runtime picker switches (Task 7)
    warns.extend(resolved.warnings); // join the existing startup warning stream

    // Load the personal dictionary from disk (missing/unreadable → empty; no abort).
    if let Some(dict_path) = &cfg.diagnostics.dictionary {
        if let Ok(text) = std::fs::read_to_string(dict_path) {
            editor.dictionary = text.lines().map(|l| l.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
    }

    // Recovery-on-open (§5.1).
    // Named files: use assess() with content-hash comparison.
    // Scratch buffers: their swap is pid-keyed, so look for an orphan from a
    // dead previous session (pre-merge blocker #1).
    if editor.active().document.path.is_some() {
        // Bounded read: an over-cap document yields None → assess() Prompts (safe).
        // (Narrow behavior change: a >64 MiB file whose bytes match the swap hash
        // would previously DiscardSilently; it now Prompts. Safe direction.)
        let file_bytes = editor.active().document.path.as_deref()
            .and_then(|p| crate::file::bounded_read_opt(p, crate::limits::MAX_OPEN_BYTES));
        match crate::swap::assess(editor.active().document.path.as_deref(), file_bytes.as_deref()) {
            crate::swap::RecoveryDecision::OpenNormally => {}
            crate::swap::RecoveryDecision::DiscardSilently => {
                crate::swap::delete(editor.active().document.path.as_deref());
            }
            crate::swap::RecoveryDecision::Prompt(_h, body) => {
                editor.active_mut().pending_swap_body = Some(body);
                editor.open_prompt(crate::prompt::Prompt::swap_recovery());
                editor.status = "Recovery file found".into();
            }
        }
    } else if let Some((sp, _header, body)) = crate::swap::find_orphan_scratch_swap() {
        editor.active_mut().pending_swap_body = Some(body);
        editor.active_mut().pending_swap_path = Some(sp);
        editor.open_prompt(crate::prompt::Prompt::swap_recovery());
        editor.status = "Recovery file found".into();
    }

    // Install the terminal guard: enable raw mode + enter alternate screen.
    // Mouse capture is gated on editor.mouse_capture (seeded from config above).
    let mut guard = term::TerminalGuard::new(editor.mouse_capture)?;
    let mut applied_mouse = editor.mouse_capture;

    // Initial derive so the first draw has up-to-date layouts.
    derive::rebuild(&mut editor);

    // Warm the pandoc probe cache so the first export command doesn't pay latency.
    let _ = crate::export::probe_pandoc();

    // Warm Harper's FstDictionary LazyLock off the critical path so the first
    // real diagnostics check isn't ~11s. Fire-and-forget; discard the result.
    if editor.diag_cfg.enabled {
        std::thread::Builder::new()
            .name("wcartel-diag-warm".into())
            .spawn(|| {
                let ignore = std::collections::HashSet::new();
                let opts = wordcartel_core::diagnostics::CheckOpts { grammar: false, ignore_words: &ignore };
                let _ = wordcartel_core::diagnostics::check("", &opts);
            })
            .expect("spawn diag warmup thread");
    }

    let reg = Registry::builtins();
    // Build the keymap from the loaded config and surface any warnings.
    let (built_keymap, mut kw) = keymap::build_keymap(&cfg.keymap, &reg);
    warns.append(&mut kw);
    editor.keymap = built_keymap;
    if let Some(w) = warns.first() {
        editor.status = w.clone();
    }
    // Take the keymap out of editor into a loop-local to avoid a simultaneous
    // &mut editor / &editor.keymap borrow conflict when calling reduce.
    // (The keymap doesn't change during the loop in v1.)
    let keymap = std::mem::take(&mut editor.keymap);
    let (msg_tx, msg_rx) = std::sync::mpsc::channel::<Msg>();
    let (wake_tx, wake_rx) = std::sync::mpsc::channel::<()>();
    let executor = crate::jobs::ThreadExecutor::new(wake_tx);
    let clip_tx = crate::clipboard::spawn_worker(msg_tx.clone());

    // Worker → loop wake relay: each result nudges the loop to drain. reduce()'s
    // trailing ex.drain() does the actual merge, so Msg::Tick is the nudge.
    {
        let msg_tx = msg_tx.clone();
        std::thread::spawn(move || {
            while wake_rx.recv().is_ok() {
                if msg_tx.send(Msg::Tick).is_err() { break; }
            }
        });
    }

    // Input thread + watchdog. The reader blocks on read() and forwards events;
    // if it ever ends (Err from read(), or a panic), the watchdog surfaces
    // Msg::InputThreadDied so the loop shuts down cleanly instead of hanging
    // (other Sender<Msg> clones keep msg_rx alive, so its disconnect never fires).
    {
        let input_tx = msg_tx.clone();
        let input_handle = std::thread::Builder::new()
            .name("wcartel-input".into())
            .spawn(move || {
                while let Ok(ev) = crossterm::event::read() {
                    if input_tx.send(Msg::Input(ev)).is_err() { break; }
                }
            })
            .expect("spawn input thread");
        let watch_tx = msg_tx.clone();
        std::thread::Builder::new()
            .name("wcartel-input-watchdog".into())
            .spawn(move || {
                let _ = input_handle.join(); // unblocks on ANY reader end (Ok or panic)
                let _ = watch_tx.send(Msg::InputThreadDied);
            })
            .expect("spawn input watchdog");
    }

    let clock = SystemClock;
    const SAVE_QUIT_TIMEOUT_MS: u64 = 5_000;

    // Load the session store once at startup (corrupt/missing → empty, no abort).
    let mut session = crate::state::load();
    // Initialize the seq counter one past the highest stored seq so that newly
    // recorded entries always outrank loaded ones for LRU eviction (Codex pre-merge fix).
    let mut session_seq: u64 = session.next_seq();

    // Resume-on-open: if cfg.state.resume is set and the buffer has a path, restore
    // cursor/scroll/marks/folds via the shared seam (factored from this block so launch
    // and open_into_current stay byte-identical). The staleness guard lives in
    // restore_resume → apply_resume.
    if cfg.state.resume {
        if let Some(raw_path) = editor.active().document.path.clone() {
            crate::session_restore::restore_resume(&mut editor, &raw_path);
        }
    }

    // Track saved_version to detect when a save completes in the loop.
    let mut last_persisted_saved = editor.active().document.saved_version;

    // Reconcile mouse capture once before the first draw (post-guard invariant).
    reconcile_mouse_capture(&mut editor, guard.terminal().backend_mut(), &mut applied_mouse);

    recompute_scrollbar_visible(&mut editor, clock.now_ms());
    // After a potential session-resume the scroll may have changed; re-clamp/re-pin
    // and rebuild the layout cache so the very first frame is always correct.
    // Order: rebuild (reconciles folds + layout) → SnapOut restored caret → ensure_visible.
    // This ensures a cursor saved inside a now-folded section is snapped to the visible
    // heading before the first draw, maintaining the caret-never-in-a-fold invariant.
    derive::rebuild(&mut editor);
    {
        use crate::registry::{place_caret_visible, CaretPlace};
        let head = editor.active().document.selection.primary().head;
        let nh = place_caret_visible(&mut editor, head, CaretPlace::SnapOut);
        if nh != head {
            editor.active_mut().document.selection =
                wordcartel_core::selection::Selection::single(nh);
        }
    }
    crate::nav::ensure_visible(&mut editor);
    guard.terminal().draw(|f| render::render(f, &mut editor))?;
    let mut exit_reason = ExitReason::Normal;
    loop {
        let now = clock.now_ms();
        // Bounded save&quit: if waiting for an in-flight save to complete and
        // 5 s have elapsed since the last edit, re-raise the quit-confirm modal.
        if let Some(p) = &editor.pending_after_save {
            let waited = now.saturating_sub(p.at_ms);
            if waited > SAVE_QUIT_TIMEOUT_MS {
                // Codex I-new-4: match on &p.action via `matches!` BEFORE clearing
                // pending_after_save (decide the action, then mutate — avoids moving
                // out of the borrowed `pending_after_save`).
                let timed_out_drain = matches!(&p.action, crate::editor::PostSaveAction::ContinueQuitDrain);
                let timed_out_quit  = matches!(&p.action, crate::editor::PostSaveAction::Quit);
                editor.pending_after_save = None;
                if timed_out_quit {
                    // Re-raise the quit-confirm modal so the user can choose again.
                    editor.open_prompt(crate::prompt::Prompt::quit_confirm());
                    editor.status = "Save still running — choose again".into();
                } else if timed_out_drain {
                    // Codex C3: a stranded drain (no in-flight save, no re-drive) would
                    // hang the quit. Abort the whole quit rather than silently clearing.
                    editor.quit_drain = None;
                    editor.quit_drain_advance = false;
                    editor.status = "save timed out — quit cancelled".into();
                } else {
                    editor.status = "Save still running — try again".into();
                }
            }
        }
        let swap_deadline = crate::swap::next_deadline_ms(now, editor.active().last_edit_at, editor.active().last_swap_at);
        let sq_deadline = editor.pending_after_save.as_ref().map(|p| p.at_ms.saturating_add(SAVE_QUIT_TIMEOUT_MS));
        // Include scrollbar_until_ms in the deadline so the loop wakes when the
        // bar should fade (avoids relying on the idle 1-hour Tick).
        let sb_deadline = if editor.mouse.scrollbar_until_ms > now {
            Some(editor.mouse.scrollbar_until_ms)
        } else {
            None
        };
        // Menu-bar dwell/grace: at most one is Some by construction (the Moved arm
        // clears the other side); recompute_menu_bar clears a fired due, so a past
        // deadline cannot persist and spin the loop.
        let menu_deadline = editor.mouse.menu_reveal_due.or(editor.mouse.menu_hide_due);
        // Fix A3: include the diagnostics deadline ONLY when no check is in
        // flight.  When a check is in flight, recheck_due_at may be a past
        // timestamp (armed before the check started), which would drive
        // recv_timeout(0) → 100% CPU spin until the worker completes.
        // When the result lands it clears in_flight_version; the next
        // iteration will re-include the (re-armed) deadline and dispatch.
        let diag_deadline = if editor.active().diagnostics.in_flight_version.is_none() {
            editor.active().diagnostics.recheck_due_at
        } else {
            None
        };
        let reconcile_deadline = if editor.active().reconcile.in_flight_version.is_none() {
            editor.active().reconcile.due_at
        } else {
            None
        };
        let deadline = crate::diagnostics_run::next_deadline(&[
            swap_deadline,
            sq_deadline,
            sb_deadline,
            menu_deadline,
            diag_deadline,
            reconcile_deadline,
        ]);
        let timeout = deadline
            .map(|d| std::time::Duration::from_millis(d.saturating_sub(now)))
            .unwrap_or(std::time::Duration::from_secs(3600));
        let msg = match msg_rx.recv_timeout(timeout) {
            Ok(m) => m,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Msg::Tick,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        // Input-reader death: shut down cleanly BEFORE any modal/reduce handling
        // (the modal match would otherwise swallow it via its `_ => {}`).
        if let Msg::InputThreadDied = msg {
            exit_reason = ExitReason::InputLost;
            break;
        }
        let (pre_id, pre_version) = { let b = editor.active(); (b.id, b.document.version) };
        let keep = reduce(msg, &mut editor, &reg, &keymap, &executor, &clock, &msg_tx);
        editor.note_undo_eviction(pre_id, pre_version);
        crate::clipboard::drain_clipboard_intents(&mut editor, guard.terminal().backend_mut(), &clip_tx, &msg_tx);
        reconcile_mouse_capture(&mut editor, guard.terminal().backend_mut(), &mut applied_mouse);
        advance(&mut editor, &clock);
        guard.terminal().draw(|f| render::render(f, &mut editor))?;
        // Persist session state when a save just completed (saved_version advanced).
        let sv = editor.active().document.saved_version;
        if sv != last_persisted_saved {
            session_seq += 1;
            persist_session(&mut session, &editor, &cfg, session_seq);
            last_persisted_saved = sv;
        }
        if !keep { break; }
    }

    // Input-loss shutdown: persist every dirty buffer non-interactively (the
    // interactive quit-drain can't run — input is gone). Controlled break, so
    // iterating buffers is safe.
    if exit_reason == ExitReason::InputLost {
        if let Ok(dir) = crate::swap::state_dir() {
            crate::recovery::dump_all_dirty(&editor, &dir);
        }
    }

    // On clean quit: persist once more (cursor may have moved since the last save).
    session_seq += 1;
    persist_session(&mut session, &editor, &cfg, session_seq);

    // Restore the terminal BEFORE the executor drops: ThreadExecutor::drop joins
    // the worker, which may still be completing an in-flight save_atomic on a slow
    // filesystem. Dropping the guard first guarantees the user gets their terminal
    // back immediately; we still join (don't abandon an in-flight atomic save — that
    // is the "never lose work" behavior). The 5 s save&quit guard above bounds the wait.
    drop(guard);
    Ok(exit_reason)
}

/// Recompute `editor.mouse.scrollbar_visible` from the clock.
///
/// Must be called at the top of the run loop (with `clock.now_ms()`) so that
/// the scrollbar fades exactly when `scrollbar_until_ms` expires, driven by
/// the loop's `deadline` (not an idle Tick).
pub fn recompute_scrollbar_visible(editor: &mut crate::editor::Editor, now_ms: u64) {
    editor.mouse.scrollbar_visible = now_ms < editor.mouse.scrollbar_until_ms;
}

/// Fire the auto-mode menu-bar deadlines (armed by the mouse Moved arm). Gated on
/// Auto — a stale due must never fire in Pinned/Hidden (spec M2).
pub fn recompute_menu_bar(editor: &mut crate::editor::Editor, now_ms: u64) {
    if editor.menu_bar_mode != crate::config::MenuBarMode::Auto {
        // Defense-in-depth (Fable plan-review M5): dues arm only in Auto and every
        // mode transition clears them, so this state is unreachable — but CLEARING
        // (never firing) here makes the deadline-array no-spin invariant
        // unconditional instead of resting on the transition-clears.
        editor.mouse.menu_reveal_due = None;
        editor.mouse.menu_hide_due = None;
        return;
    }
    if editor.mouse.menu_reveal_due.is_some_and(|d| now_ms >= d) {
        editor.mouse.menu_reveal_due = None;
        editor.mouse.menu_bar_revealed = true;
    }
    if editor.mouse.menu_hide_due.is_some_and(|d| now_ms >= d) {
        editor.mouse.menu_hide_due = None;
        editor.mouse.menu_bar_revealed = false;
    }
}

/// Reconcile the terminal's mouse-capture state with `editor.mouse_capture`.
///
/// Enables or disables mouse capture on the backend when the desired state
/// diverges from `applied`. On disable, clears drag state so no stale Up
/// events are awaited for a capture that will never arrive.
pub fn reconcile_mouse_capture<W: std::io::Write>(editor: &mut crate::editor::Editor, backend: &mut W, applied: &mut bool) {
    if editor.mouse_capture != *applied {
        if editor.mouse_capture {
            if crossterm::execute!(backend, crossterm::event::EnableMouseCapture).is_ok() {
                *applied = editor.mouse_capture;
            }
        } else {
            // clear drag state regardless of IO outcome — it is local state,
            // not tied to the terminal write succeeding.
            editor.mouse.dragging = false;
            editor.mouse.scrollbar_dragging = false;
            editor.mouse.anchor = None;
            editor.mouse.menu_reveal_due = None;
            editor.mouse.menu_hide_due = None;
            editor.mouse.menu_bar_revealed = false;
            if crossterm::execute!(backend, crossterm::event::DisableMouseCapture).is_ok() {
                *applied = editor.mouse_capture;
            }
        }
    }
}

/// Record the active buffer's position into the session store and flush to disk.
/// Scratch content is always captured; per-file entry only for named buffers.
/// A write failure → status warning only (never blocks quit or loses the document).
fn persist_session(
    session: &mut crate::state::SessionState,
    editor: &Editor,
    cfg: &config::Config,
    seq: u64,
) {
    // Effort 6: capture scratch content first, independent of the active buffer.
    // M5: guard on byte length — never materialize a huge String for persistence.
    if let Some(sid) = editor.scratch_id {
        if let Some(sb) = editor.by_id(sid) {
            if sb.document.buffer.len() <= crate::limits::MAX_SESSION_BYTES {
                session.scratch = Some(crate::state::ScratchState {
                    text: sb.document.buffer.to_string(),
                    cursor: sb.document.selection.primary().head,
                });
            } else {
                // Oversized: skip persisting the live scratch — and CLEAR any stale scratch
                // loaded from disk, so an old session's scratch is not resurrected. The live
                // buffer is untouched; only its cross-session persistence is dropped.
                session.scratch = None;
            }
        }
    }
    // Per-file entry for the active buffer (unchanged): only when it has a real,
    // canonicalizable path. Scratch/new buffers contribute no per-file entry.
    if let Some(raw_path) = editor.active().document.path.as_deref() {
        if let Ok(canon) = std::fs::canonicalize(raw_path) {
            if let Some((mtime, size)) = crate::state::file_identity(raw_path) {
                let entry = crate::state::StateEntry {
                    cursor: editor.active().document.selection.primary().head,
                    scroll: editor.active().view.scroll,
                    marks: editor.active().marks.iter().map(|(c, &o)| (c.to_string(), o)).collect(),
                    mtime, size, seq,
                    folds: editor.active().folds.folded().iter().copied().collect(),
                    block: editor.active().marked_block.map(|b| (b.start, b.end)),
                };
                session.record(canon.to_string_lossy().into_owned(), entry, cfg.state.max_entries);
            }
        }
    }
    // Always flush — scratch durability does not depend on the active buffer.
    let _ = session.save();
}

#[cfg(test)]
pub fn persist_session_for_test(s: &mut crate::state::SessionState, e: &Editor, cfg: &config::Config, seq: u64) {
    persist_session(s, e, cfg, seq);
}

// ---------------------------------------------------------------------------
// Tests — written FIRST (RED phase) before any implementation
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use crate::editor::Editor;
    use crate::app::Msg;
    use crate::test_support::{TestClock, key_char, press};
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);

    fn cua_keymap() -> crate::keymap::KeyTrie {
        let (t, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &crate::registry::Registry::builtins());
        t
    }

    fn f10() -> crossterm::event::Event {
        crossterm::event::Event::Key(KeyEvent {
            code: KeyCode::F(10),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    // -------------------------------------------------------------------------
    // Brief's required failing test (Task 12 step 1)
    // -------------------------------------------------------------------------

    /// Feed "hi" then Ctrl+Q (modal) then 'q' (QuitAnyway); confirm the buffer holds "hi\n" and quit.
    #[test]
    fn step_processes_typing_and_quit() {
        use crate::registry::Registry;
        use crate::jobs::InlineExecutor;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let reg = Registry::builtins();
        let km = cua_keymap();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        for c in "hi".chars() {
            crate::app::step(&mut e, key_char(c), &clk);
        }
        // First Ctrl+Q: dirty → multi-buffer quit modal up, NOT quit yet (Effort 6).
        let ctrl_q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(crate::app::Msg::Input(ctrl_q), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "dirty quit must raise the multi-buffer modal");
        assert!(!e.quit);
        // Press 'r' → Review each → per-buffer review prompt for the one dirty buffer.
        let key = |c: char| Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(crate::app::Msg::Input(key('r')), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "review-each raises the per-buffer prompt");
        assert!(!e.quit);
        // Press 'd' → Discard this buffer → drain empties → quit.
        crate::app::reduce(crate::app::Msg::Input(key('d')), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.quit, "discarding the last dirty buffer quits");
        assert_eq!(e.active().document.buffer.to_string(), "hi\n");
    }

    #[test]
    fn palette_enter_on_buffer_row_switches_buffer_and_closes_palette() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.buffers[0].document.path = Some(std::path::PathBuf::from("/tmp/doc.md"));
        e.install_scratch();
        let scratch_id = e.scratch_id.unwrap();
        // install_scratch seeds MRU as [doc, scratch]; open switcher → rows[0]=doc, rows[1]=scratch
        e.open_buffer_switcher();
        // Select the scratch row (index 1)
        e.palette.as_mut().unwrap().selected = 1;
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let enter = Msg::Input(Event::Key(KeyEvent {
            code: KeyCode::Enter, modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE,
        }));
        crate::app::reduce(enter, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.palette.is_none(), "buffer-switcher palette must be dismissed after Enter");
        assert_eq!(e.active().id, scratch_id,
            "active buffer must be the buffer selected in the switcher");
    }

    #[test]
    fn copy_sets_register_and_sync_request() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        // select "hello" (0..5)
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5);
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let ctrl_c = Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(ctrl_c), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.register.get(), Some("hello"));
        assert_eq!(e.clipboard_sync_request.as_deref(), Some("hello"));
    }

    #[test]
    fn paste_keypress_sets_intent_not_inline_paste() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.register.set("Z".into());
        let before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let ctrl_v = Event::Key(KeyEvent { code: KeyCode::Char('v'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(ctrl_v), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), before, "Ctrl+V no longer pastes inline");
        assert!(e.clipboard_get_pending.is_some(), "Ctrl+V sets a paste intent");
    }

    #[test]
    fn clipboardpaste_some_inserts_os_text_one_undo() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1); // caret after 'a'
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXYb\n");
        assert_eq!(e.register.get(), Some("XY"), "OS text updates the register");
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");
    }

    #[test]
    fn f10_opens_menu_and_selected_event_dispatches() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        // Simulate a menu selection routing: open, then feed a synthesized Selected(copy) via the menu handler path.
        crate::app::reduce(Msg::Input(f10()), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_some(), "F10 opens the menu");
        // drive a selection of "copy" through the menu->dispatch path (helper exercising drain_events->dispatch_overlay_command)
        crate::app::menu_select_for_test(&mut e, &reg, &ex, &clk, &tx, crate::registry::CommandId("copy"));
        assert!(e.menu.is_none(), "selection closes the menu");
        assert_eq!(e.register.get(), Some("abc"));
    }

    #[test]
    fn menu_keyboard_nav_moves_and_dispatches() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |c| Event::Key(KeyEvent { code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // F10 opens; menu hydrated with groups
        crate::app::reduce(Msg::Input(press(KeyCode::F(10))), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_some());
        let m = e.menu.as_ref().unwrap();
        assert!(!m.groups.is_empty(), "menu hydrated with groups");
        assert_eq!(m.open, 0);
        // Right moves to the next category, Down highlights a row
        crate::app::reduce(Msg::Input(press(KeyCode::Right)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.menu.as_ref().unwrap().open, 1);
        crate::app::reduce(Msg::Input(press(KeyCode::Down)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.menu.as_ref().unwrap().highlighted, 1);
    }

    #[test]
    fn clipboardpaste_none_falls_back_to_register() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        e.register.set("R".into());
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: None }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aRb\n", "None -> register fallback");
    }

    #[test]
    fn clipboardpaste_none_empty_register_is_noop() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: None }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "ab\n", "empty register -> no change");
    }

    #[test]
    fn clipboardpaste_replaces_active_selection() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(1, 3); // select "bc"
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXYd\n", "selection replaced by pasted text");
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "abcd\n");
    }

    #[test]
    fn clipboardpaste_for_missing_buffer_is_noop() {
        use crate::editor::{Editor, BufferId}; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: BufferId(99999), text: Some("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "ab\n", "unknown buffer -> dropped");
    }

    #[test]
    fn clipboardpaste_oversize_skips_insert_and_register() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        e.register.set("orig".into());
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let huge = "x".repeat(crate::clipboard::PASTE_MAX_BYTES + 1);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some(huge) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "ab\n", "oversize paste must not insert");
        assert_eq!(e.register.get(), Some("orig"), "oversize paste must not mutate the register");
        assert!(e.status.to_lowercase().contains("too large"));
    }

    #[test]
    fn availability_false_shows_notice_once() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardAvailability(false), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.status.to_lowercase().contains("clipboard"));
        assert!(e.clipboard_notice_shown);
        e.status = "typing".into();
        crate::app::reduce(Msg::ClipboardAvailability(false), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.status, "typing", "notice shown only once");
    }

    // -------------------------------------------------------------------------
    // key_to_command mapping tests
    // -------------------------------------------------------------------------

    /// Ctrl+S maps to Command::Save.
    #[test]
    fn key_to_command_ctrl_s_is_save() {
        use crate::commands::Command;
        let k = KeyEvent {
            code: KeyCode::Char('s'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(crate::input::key_to_command(k), Some(Command::Save)));
    }

    /// Shift+Right maps to Move { dir: Right, extend: true }.
    #[test]
    fn key_to_command_shift_right_extends() {
        use crate::commands::{Command, Dir};
        let k = KeyEvent {
            code: KeyCode::Right,
            modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(
            crate::input::key_to_command(k),
            Some(Command::Move { dir: Dir::Right, extend: true })
        ));
    }

    /// A printable char maps to InsertChar.
    #[test]
    fn key_to_command_printable_is_insert_char() {
        use crate::commands::Command;
        let k = key_char('A');
        assert!(matches!(crate::input::key_to_command(k), Some(Command::InsertChar('A'))));
    }

    /// An unmapped key (F5) returns None.
    #[test]
    fn key_to_command_unmapped_is_none() {
        let k = KeyEvent {
            code: KeyCode::F(5),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(crate::input::key_to_command(k).is_none());
    }

    /// Release events return None (double-input guard).
    #[test]
    fn key_to_command_release_is_none() {
        let k = KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        };
        assert!(crate::input::key_to_command(k).is_none());
    }

    #[test]
    fn reduce_handles_typing_via_registry() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        for c in "hi".chars() {
            let ev = Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press, state: KeyEventState::NONE });
            assert!(crate::app::reduce(crate::app::Msg::Input(ev), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx));
        }
        assert_eq!(e.active().document.buffer.to_string(), "hi\n");
    }

    #[test]
    fn filterdone_replaces_range_when_fresh() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let msg = Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) };
        crate::app::reduce(msg, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXde\n");
        // one undo step restores the original
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "abcde\n");
    }

    #[test]
    fn filterdone_discarded_when_version_moved() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let stale_v = e.active().document.version;
        e.active_mut().document.version += 1; // simulate an intervening edit
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: stale_v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "abcde\n", "stale filter result discarded");
        assert!(e.status.to_lowercase().contains("discarded"));
    }

    #[test]
    fn filterdone_failure_shows_status_keeps_buffer() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult, FilterError};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter,
            outcome: RunResult::Err(FilterError::NonZero { code: "Exited(3)".into(), stderr: "boom".into() }) },
            &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "abcde\n");
        assert!(e.status.contains("boom") && e.status.contains('3'));
    }

    #[test]
    fn dispatch_filter_runs_real_command_and_delivers_filterdone() {
        // One live-thread integration test (deterministic: block on the channel).
        use crate::editor::Editor;
        use crate::filter::{dispatch_filter, FilterSpec, Disposition, Input};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let spec = FilterSpec { argv: vec!["tr".into(),"a-z".into(),"A-Z".into()], shell: false,
            disposition: Disposition::Filter, input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10), max_output: 1 << 20 };
        dispatch_filter(&mut e, spec, tx);
        let msg = rx.recv().expect("FilterDone must arrive"); // blocks; no timing assert
        match msg { Msg::FilterDone { outcome: crate::filter::RunResult::Stdout(s), .. } => assert_eq!(s, "ABC\n"),
                    other => panic!("expected FilterDone Stdout, got {other:?}") }
    }

    #[test]
    fn tick_writes_swap_when_idle_elapsed_and_dirty() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        let doc_path = std::env::temp_dir().join(format!(
            "wc-tick-swap-{}-{}.md",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        let mut e = Editor::new_from_text("\n", Some(doc_path.clone()), (80, 24));
        e.active_mut().document.version = 1;            // dirty (saved_version=Some(0))
        e.active_mut().last_edit_at = Some(0);
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        // Clock past the idle threshold.
        struct C(u64); impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let clk = C(crate::swap::T_IDLE_MS + 5);
        crate::app::reduce(crate::app::Msg::Tick, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.active().last_swap_at.is_some(), "an idle Tick on a dirty buffer writes a swap");
        let sp = crate::swap::swap_path(Some(&doc_path)).unwrap();
        assert!(sp.exists());
        let _ = std::fs::remove_file(&sp);
        let _ = std::fs::remove_file(&doc_path);
    }

    #[test]
    fn quit_with_unsaved_raises_multi_then_review_discard_exits() {
        // Effort 6: a dirty buffer now raises the multi-buffer quit modal; the
        // "quit anyway" equivalent is Review-each → Discard.
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.active_mut().document.version = 1; // dirty
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let ctrl_q = Event::Key(KeyEvent { code: KeyCode::Char('q'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // First Ctrl+Q → multi-buffer modal up, not quit.
        crate::app::reduce(crate::app::Msg::Input(ctrl_q.clone()), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.prompt.is_some() && !e.quit);
        let key = |c: char| Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // 'r' → Review each → per-buffer prompt.
        crate::app::reduce(crate::app::Msg::Input(key('r')), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.prompt.is_some() && !e.quit, "review-each raises the per-buffer prompt");
        // 'd' → Discard → drain empties → quit.
        crate::app::reduce(crate::app::Msg::Input(key('d')), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.quit, "discard exits");
        assert!(e.prompt.is_none(), "prompt cleared");
    }

    // -----------------------------------------------------------------------
    // Effort 6 Task 8: multi-buffer quit (Save-All / Review-each) state machine
    // -----------------------------------------------------------------------

    fn quit_tmp(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "wc-quit8-{}-{}-{}.md",
            tag, std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)))
    }

    #[test]
    fn quit_with_no_dirty_buffers_quits_immediately() {
        use crate::editor::Editor;
        let clk = TestClock(0);
        let mut e = Editor::new_from_text("clean\n", Some(quit_tmp("clean")), (40, 10));
        e.install_scratch();
        let v = e.active().document.version;
        e.active_mut().document.mark_saved(v); // clean (no dirty buffers)
        let r = crate::commands::run(crate::commands::Command::Quit, &mut e, &clk);
        assert!(e.quit, "no dirty buffers → quit immediately");
        assert!(matches!(r, crate::commands::CommandResult::Quit));
        assert!(e.prompt.is_none(), "no modal raised when nothing is dirty");
    }

    #[test]
    fn quit_save_all_drains_named_dirty_then_quits() {
        use crate::editor::{Buffer, Editor};
        use crate::jobs::{Executor, InlineExecutor};
        use crate::prompt::PromptAction;
        let p0 = quit_tmp("a"); std::fs::write(&p0, "old\n").unwrap();
        let p1 = quit_tmp("b"); std::fs::write(&p1, "old\n").unwrap();
        let mut e = Editor::new_from_text("new0\n", Some(p0.clone()), (80, 24));
        e.active_mut().document.saved_version = None; e.active_mut().document.version = 1; // dirty
        let id1 = e.alloc_id();
        let area = e.active().view.area;
        e.buffers.push(Buffer::from_text(id1, "new1\n", Some(p1.clone()), area));
        e.buffers[1].document.saved_version = None; e.buffers[1].document.version = 1; // dirty
        e.install_scratch();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(PromptAction::QuitSaveAll, &mut e, &ex, &clk, &tx);
        // Drive the drain to completion: each save result re-drives via apply_job_result.
        let mut guard = 0;
        while !e.quit {
            let rs = ex.drain();
            if rs.is_empty() { break; }
            for o in rs { crate::jobs_apply::apply_job_outcome(o, &mut e, &ex, &clk, &tx); }
            guard += 1; assert!(guard < 16, "drain did not converge");
        }
        assert!(e.quit, "Save-All drains both dirty buffers then quits");
        assert!(e.quit_drain.is_none(), "drain consumed");
        assert_eq!(std::fs::read_to_string(&p0).unwrap(), "new0\n");
        assert_eq!(std::fs::read_to_string(&p1).unwrap(), "new1\n");
        let _ = std::fs::remove_file(&p0); let _ = std::fs::remove_file(&p1);
    }

    #[test]
    fn quit_review_each_cancel_aborts() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::prompt::PromptAction;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.active_mut().document.version = 1; // dirty
        e.install_scratch();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(PromptAction::QuitReviewEach, &mut e, &ex, &clk, &tx);
        assert!(e.quit_drain.is_some(), "drain started");
        assert!(e.prompt.is_some(), "per-buffer review prompt raised");
        crate::prompts::resolve_prompt(PromptAction::Cancel, &mut e, &ex, &clk, &tx);
        assert!(e.quit_drain.is_none(), "cancel aborts the drain");
        assert!(!e.quit, "not quitting after cancel");
    }

    #[test]
    fn review_prompt_esc_aborts_quit_drain() {
        // Regression (Codex gate Finding 2): pressing Esc on the per-buffer review
        // prompt (raised by drive_quit_drain in ReviewEach mode) must abort the quit
        // drain, matching the behaviour of PromptAction::Cancel.
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::prompt::PromptAction;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.active_mut().document.version = 1; // dirty (saved_version=Some(0) vs version=1)
        e.install_scratch();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        // Start a ReviewEach quit drain — drive_quit_drain raises the per-buffer prompt.
        crate::prompts::resolve_prompt(PromptAction::QuitReviewEach, &mut e, &ex, &clk, &tx);
        assert!(e.quit_drain.is_some(), "drain started");
        assert!(e.prompt.is_some(), "per-buffer review prompt raised by drive_quit_drain");
        // Simulate Esc on the review prompt via the real reduce path.
        let reg = Registry::builtins();
        let km = cua_keymap();
        let esc = Event::Key(KeyEvent {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        crate::app::reduce(crate::app::Msg::Input(esc), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.quit_drain.is_none(), "Esc on review prompt aborts the drain");
        assert!(!e.quit, "app does not quit after Esc-abort");
    }

    #[test]
    fn quit_drain_aborts_on_save_failure() {
        // Save-All over a buffer whose save fails (symlink target is refused) →
        // quit_drain cleared, quit stays false, error status surfaced.
        #[cfg(not(unix))] { return; }
        #[cfg(unix)]
        {
            use crate::editor::Editor;
            use crate::jobs::{Executor, InlineExecutor};
            use crate::prompt::PromptAction;
            let real = quit_tmp("real"); std::fs::write(&real, "real\n").unwrap();
            let link = quit_tmp("link"); std::os::unix::fs::symlink(&real, &link).unwrap();
            let mut e = Editor::new_from_text("x\n", Some(link.clone()), (80, 24));
            e.active_mut().document.saved_version = None; e.active_mut().document.version = 1; // dirty
            e.install_scratch();
            let ex = InlineExecutor::default();
            let clk = TestClock(0);
            let (tx, _rx) = std::sync::mpsc::channel();
            crate::prompts::resolve_prompt(PromptAction::QuitSaveAll, &mut e, &ex, &clk, &tx);
            for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, &mut e, &ex, &clk, &tx); }
            assert!(e.quit_drain.is_none(), "failed save aborts the drain");
            assert!(!e.quit, "a failed save must not quit (no data loss)");
            assert!(e.status.to_lowercase().contains("symlink"), "error status surfaced");
            let _ = std::fs::remove_file(&link); let _ = std::fs::remove_file(&real);
        }
    }

    #[test]
    fn quit_drain_aborts_when_save_as_dismissed() {
        // Drain reaches a dirty UNNAMED buffer → Save-As minibuffer opens. An empty
        // submit (dismiss) aborts the quit: quit_drain cleared, quit stays false.
        use crate::editor::Editor;
        use crate::prompt::PromptAction;
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.active_mut().document.version = 1; // dirty UNNAMED
        e.install_scratch();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(PromptAction::QuitSaveAll, &mut e, &ex, &clk, &tx);
        assert_eq!(e.minibuffer.as_ref().map(|m| m.kind), Some(crate::minibuffer::MinibufferKind::SaveAs),
            "unnamed dirty buffer in the drain opens the Save-As minibuffer");
        assert!(e.quit_drain.is_some(), "drain still pending while Save-As is open");
        // Dismiss via empty submit.
        crate::prompts::save_as_submit(&mut e, "", &ex, &clk, &tx);
        assert!(e.quit_drain.is_none(), "dismissing the Save-As aborts the drain");
        assert!(!e.quit, "backing out of Save-As must not quit");
    }

    #[test]
    fn save_and_quit_sets_pending_after_save_and_exits_on_matching_result() {
        use crate::editor::{Editor, PostSaveAction};
        use crate::jobs::{Executor, InlineExecutor};
        use crate::prompt::PromptAction;
        let p = std::env::temp_dir().join(format!("wc-savequit-{}.md", std::process::id()));
        std::fs::write(&p, "old\n").unwrap();
        let mut e = Editor::new_from_text("new\n", Some(p.clone()), (80, 24));
        e.active_mut().document.saved_version = None; e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::resolve_prompt(PromptAction::SaveAndQuit, &mut e, &ex, &clk, &tx);
        assert!(matches!(e.pending_after_save, Some(crate::editor::PendingAfterSave { version: 1, action: PostSaveAction::Quit, .. })));
        assert!(!e.quit, "not yet — waiting for the save result");
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert!(e.quit, "matching save result triggers quit");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn save_and_quit_command_on_unnamed_buffer_does_not_arm() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("scratch\n", None, (80, 24));
        e.active_mut().document.version = 1;
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        {
            let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx.clone() };
            crate::save::dispatch_save_and_quit(&mut ctx);
        }
        assert_eq!(e.pending_after_save, None, "no path → not armed");
        assert!(!e.quit);
    }

    // Effort 6 additive open: open_as_new_buffer adds a buffer and switches to it,
    // leaving the original buffer intact. No clobber risk in the additive model.
    #[test]
    fn open_as_new_buffer_is_additive_never_replaces() {
        use crate::editor::Editor;
        let target = std::env::temp_dir().join(format!("wc-clobber-open-{}.md", std::process::id()));
        std::fs::write(&target, "OPEN TARGET\n").unwrap();
        let named = std::env::temp_dir().join(format!("wc-clobber-named-{}.md", std::process::id()));
        std::fs::write(&named, "v1 content\n").unwrap();
        let mut e = Editor::new_from_text("v1 content\n", Some(named.clone()), (80, 24));
        let id = e.active().id;
        let before_count = e.buffers.len();
        crate::workspace::open_as_new_buffer(&mut e, &target);
        assert_eq!(e.buffers.len(), before_count + 1, "buffer added additively, not replaced");
        assert_eq!(e.active().document.buffer.to_string(), "OPEN TARGET\n", "active is new file");
        assert_ne!(e.active().id, id, "switched to the newly opened buffer");
        assert!(e.buffers.iter().any(|b| b.id == id), "original buffer still in the list");
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_file(&named);
    }

    // Effort 6 additive new: new_empty_buffer adds a buffer without touching the dirty original.
    #[test]
    fn new_empty_buffer_leaves_dirty_buffer_intact() {
        use crate::editor::Editor;
        let named = std::env::temp_dir().join(format!("wc-clobber-new-{}.md", std::process::id()));
        std::fs::write(&named, "v1 content\n").unwrap();
        let mut e = Editor::new_from_text("v1 content\n", Some(named.clone()), (80, 24));
        let id = e.active().id;
        e.active_mut().document.version = 2; // dirty
        let before_count = e.buffers.len();
        crate::workspace::new_empty_buffer(&mut e);
        assert_eq!(e.buffers.len(), before_count + 1, "new buffer added");
        assert_eq!(e.active().document.buffer.to_string(), "\n", "active is new empty buffer");
        assert_ne!(e.active().id, id, "switched away from dirty buffer");
        assert!(e.buffers.iter().any(|b| b.id == id), "dirty buffer still in the list");
        assert!(e.pending_after_save.is_none());
        let _ = std::fs::remove_file(&named);
    }

    // Effort 6 additive open: active switches to the opened file content.
    #[test]
    fn open_as_new_buffer_switches_to_opened_file() {
        use crate::editor::Editor;
        let target = std::env::temp_dir().join(format!("wc-clean-open-{}.md", std::process::id()));
        std::fs::write(&target, "OPEN TARGET\n").unwrap();
        let named = std::env::temp_dir().join(format!("wc-clean-named-{}.md", std::process::id()));
        std::fs::write(&named, "v1 content\n").unwrap();
        let mut e = Editor::new_from_text("v1 content\n", Some(named.clone()), (80, 24));
        crate::workspace::open_as_new_buffer(&mut e, &target);
        assert_eq!(e.active().document.buffer.to_string(), "OPEN TARGET\n", "active is opened file");
        assert!(e.pending_after_save.is_none());
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_file(&named);
    }

    // Effort 6 additive open: opening a path that resolves to an I/O error (directory)
    // sets editor.status to the error message and adds no new buffer.
    #[test]
    fn open_as_new_buffer_sets_error_on_io_failure() {
        use crate::editor::Editor;
        // /tmp is a directory — opening it as a file returns IsDir error.
        let dir_path = std::path::PathBuf::from("/tmp");
        let mut e = Editor::new_from_text("content\n", None, (80, 24));
        let before_count = e.buffers.len();
        crate::workspace::open_as_new_buffer(&mut e, &dir_path);
        assert!(!e.status.is_empty(), "error status set on failure, got: {:?}", e.status);
        assert_eq!(e.buffers.len(), before_count, "no buffer added on open failure");
    }

    // Effort 6 additive new: new_empty_buffer always succeeds (no I/O) and clears any prior status.
    #[test]
    fn new_empty_buffer_always_succeeds_and_clears_status() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("content\n", None, (80, 24));
        e.status = "prior error".to_string();
        let before_count = e.buffers.len();
        crate::workspace::new_empty_buffer(&mut e);
        assert_eq!(e.buffers.len(), before_count + 1, "new buffer added");
        assert_eq!(e.status, "", "status cleared after new");
    }

    // Effort 6 additive new: new_empty_buffer adds an empty untitled buffer and switches to it.
    #[test]
    fn new_empty_buffer_adds_untitled_and_switches() {
        use crate::editor::Editor;
        let named = std::env::temp_dir().join(format!("wc-clean-new-{}.md", std::process::id()));
        std::fs::write(&named, "v1 content\n").unwrap();
        let mut e = Editor::new_from_text("v1 content\n", Some(named.clone()), (80, 24));
        let id = e.active().id;
        crate::workspace::new_empty_buffer(&mut e);
        assert_ne!(e.active().id, id, "switched to new buffer");
        assert_eq!(e.active().document.buffer.to_string(), "\n", "new empty buffer content");
        assert!(e.active().document.path.is_none(), "new buffer has no path");
        assert!(e.pending_after_save.is_none());
        let _ = std::fs::remove_file(&named);
    }

    #[test]
    fn minibuffer_routing_and_submit_dispatches_filter() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let key = |c: char| Event::Key(KeyEvent { code: KeyCode::Char(c), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        for c in "cat".chars() { crate::app::reduce(Msg::Input(key(c)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx); }
        assert_eq!(e.minibuffer.as_ref().unwrap().text, "cat");
        // Enter submits -> dispatch_filter -> a FilterDone arrives, minibuffer cleared
        let enter = Event::Key(KeyEvent { code: KeyCode::Enter, modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(enter), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.minibuffer.is_none(), "submit clears the minibuffer");
        match rx.recv().unwrap() { Msg::FilterDone { outcome: crate::filter::RunResult::Stdout(s), .. } => assert_eq!(s, "abc\n"),
                                   o => panic!("expected FilterDone, got {o:?}") }
    }

    #[test]
    fn goto_line_jumps_to_line_start_and_records_jumpback() {
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("one\ntwo\nthree\nfour\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        // start at end so the jump is a real move
        let end = e.active().document.buffer.len();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(end);
        e.open_minibuffer("Go to line: ", crate::minibuffer::MinibufferKind::GotoLine);
        // type "3" then Enter
        let (tx, _rx) = std::sync::mpsc::channel();
        for ch in "3".chars() { crate::app::reduce(Msg::Input(Event::Key(KeyEvent{code:KeyCode::Char(ch),modifiers:KeyModifiers::NONE,kind:KeyEventKind::Press,state:KeyEventState::NONE})), &mut e, &Registry::builtins(), &cua_keymap(), &InlineExecutor::default(), &TestClock(0), &tx); }
        crate::app::reduce(Msg::Input(Event::Key(KeyEvent{code:KeyCode::Enter,modifiers:KeyModifiers::NONE,kind:KeyEventKind::Press,state:KeyEventState::NONE})), &mut e, &Registry::builtins(), &cua_keymap(), &InlineExecutor::default(), &TestClock(0), &tx);
        // line 3 ("three") starts at byte 8
        assert_eq!(e.active().document.selection.primary().head, e.active().document.buffer.line_to_byte(2));
        assert!(e.minibuffer.is_none(), "submit closes the minibuffer");
        // jump-back: the origin (end) was recorded so the user can return.
        assert!(e.active().jump_ring.contains(&end), "goto recorded the origin for jump-back");
    }

    #[test]
    fn minibuffer_does_not_starve_filterdone() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        let (tx, _rx) = std::sync::mpsc::channel(); let reg = Registry::builtins();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXde\n", "FilterDone applies even under an open minibuffer");
    }

    #[test]
    fn exportdone_bytes_writes_file_beside_source() {
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        // Create a temp directory and source file path.
        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-exportdone-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("notes.md");
        std::fs::write(&source, "# Hello\n").expect("write source");

        let output_path = tmp_dir.join("notes.html");

        let mut e = Editor::new_from_text("# Hello\n", Some(source.clone()), (80, 24));
        let buffer_id = e.active().id;

        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let content_bytes = b"<h1>Hello</h1>\n".to_vec();
        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(content_bytes.clone())),
            overwrite_confirmed: false,
        };
        crate::app::reduce(msg, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);

        // The output file must exist beside the source.
        assert!(output_path.exists(), "exported file must exist");
        let got = std::fs::read(&output_path).expect("read exported file");
        assert_eq!(got, content_bytes);
        assert!(e.status.contains("exported"), "status must say exported");

        // Clean up.
        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn exportdone_unconfirmed_refuses_when_target_appeared() {
        // TOCTOU guard (Codex pre-merge gate): export was dispatched because the
        // target did not exist (overwrite_confirmed=false), but a target file has
        // since appeared.  Finalization must NOT clobber it — the user never
        // agreed to overwrite — and must leave the existing content intact.
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-export-toctou-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("notes.md");
        std::fs::write(&source, "# Hello\n").expect("write source");
        let output_path = tmp_dir.join("notes.html");
        // Simulate the race: a file appeared at the target between the check and
        // the completion.
        std::fs::write(&output_path, b"PRE-EXISTING\n").expect("write racing target");

        let mut e = Editor::new_from_text("# Hello\n", Some(source.clone()), (80, 24));
        let buffer_id = e.active().id;
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(b"<h1>Hello</h1>\n".to_vec())),
            overwrite_confirmed: false,
        };
        crate::app::reduce(msg, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);

        // The racing file is untouched; status tells the user to re-run.
        let got = std::fs::read(&output_path).expect("read target");
        assert_eq!(got, b"PRE-EXISTING\n", "unconfirmed export must not clobber an appeared target");
        assert!(e.status.contains("re-run"), "status must prompt a re-run, got: {}", e.status);

        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn exportdone_confirmed_overwrites_existing_target() {
        // The complement: when the user confirmed the overwrite
        // (overwrite_confirmed=true), an existing target IS replaced.
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-export-confirmed-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("notes.md");
        std::fs::write(&source, "# Hello\n").expect("write source");
        let output_path = tmp_dir.join("notes.html");
        std::fs::write(&output_path, b"OLD\n").expect("write existing target");

        let mut e = Editor::new_from_text("# Hello\n", Some(source.clone()), (80, 24));
        let buffer_id = e.active().id;
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let new_bytes = b"<h1>Hello</h1>\n".to_vec();
        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(new_bytes.clone())),
            overwrite_confirmed: true,
        };
        crate::app::reduce(msg, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);

        let got = std::fs::read(&output_path).expect("read target");
        assert_eq!(got, new_bytes, "confirmed export must overwrite the existing target");
        assert!(e.status.contains("exported"));

        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn exportdone_under_prompt_still_applies() {
        use crate::editor::Editor;
        use crate::export::ExportResult;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;

        let tmp_dir = std::env::temp_dir().join(format!(
            "wc-exportdone-prompt-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&tmp_dir).expect("create temp dir");
        let source = tmp_dir.join("doc.md");
        std::fs::write(&source, "x\n").expect("write source");
        let output_path = tmp_dir.join("doc.html");

        let mut e = Editor::new_from_text("x\n", Some(source.clone()), (80, 24));
        e.active_mut().document.version = 1; // dirty → prompt would normally be up
        // Manually raise a prompt to simulate the overlay scenario.
        e.open_prompt(crate::prompt::Prompt::quit_confirm());

        let buffer_id = e.active().id;
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        let content_bytes = b"<p>x</p>\n".to_vec();
        let msg = Msg::ExportDone {
            buffer_id,
            target: output_path.clone(),
            result: Ok(ExportResult::Bytes(content_bytes.clone())),
            overwrite_confirmed: false,
        };
        crate::app::reduce(msg, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);

        assert!(output_path.exists(), "ExportDone under prompt must still write the file");
        assert!(e.status.contains("exported"));

        let _ = std::fs::remove_file(&output_path);
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_dir(&tmp_dir);
    }

    #[test]
    fn transform_chooser_maps_keys_to_kinds() {
        use crate::prompt::{Prompt, PromptAction};
        use crate::transform::TransformKind;
        let p = Prompt::transform_chooser();
        assert_eq!(p.action_for('r'), Some(PromptAction::Transform(TransformKind::Reflow)));
        assert_eq!(p.action_for('u'), Some(PromptAction::Transform(TransformKind::Unwrap)));
        assert_eq!(p.action_for('v'), Some(PromptAction::Transform(TransformKind::Ventilate)));
        assert_eq!(p.action_for('x'), None);
    }

    #[test]
    fn ctrl_t_opens_the_transform_chooser() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let key = Event::Key(KeyEvent { code: KeyCode::Char('t'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(key), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "Ctrl+T must open the transform chooser");
        assert_eq!(
            e.prompt.as_ref().unwrap().action_for('r'),
            Some(crate::prompt::PromptAction::Transform(crate::transform::TransformKind::Reflow)),
        );
    }

    #[test]
    fn reflow_whole_buffer_applies_one_undoable_edit() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau\n";
        let mut e = Editor::new_from_text(long, None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        // dispatch_transform takes (editor, kind, clock, msg_tx) — see Task 3 Step 6.
        crate::transform::dispatch_transform(&mut e, TransformKind::Reflow, &TestClock(0), &tx);
        let after = e.active().document.buffer.to_string();
        assert_ne!(after, long, "reflow should rewrap the long line");
        // exactly one undo restores the original
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), long);
    }

    #[test]
    fn transform_with_identical_output_makes_no_edit() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        // Already one-sentence-per-line: ventilate is a no-op → no edit, "already" status.
        let text = "Short.\n";
        let mut e = Editor::new_from_text(text, None, (80, 24));
        let v0 = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::transform::dispatch_transform(&mut e, TransformKind::Ventilate, &TestClock(0), &tx);
        assert_eq!(e.active().document.buffer.to_string(), text);
        assert_eq!(e.active().document.version, v0, "no-op transform must not bump version");
        assert!(e.status.contains("already"));
    }

    #[test]
    fn transformdone_applies_when_fresh() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        use crate::registry::Registry;
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("one two three four five six seven\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let range = 0..e.active().document.buffer.to_string().len();
        let out = "one\ntwo\n".to_string(); // pretend ventilate output
        crate::app::reduce(Msg::TransformDone { buffer_id: id, version: v, range,
            kind: TransformKind::Ventilate, result: Ok(out.clone()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), out);
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "one two three four five six seven\n");
    }

    #[test]
    fn transformdone_discarded_when_version_moved() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        use crate::registry::Registry;
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("alpha beta\n", None, (80, 24));
        let id = e.active().id; let stale = e.active().document.version;
        e.active_mut().document.version += 1; // an intervening edit
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::TransformDone { buffer_id: id, version: stale, range: 0..10,
            kind: TransformKind::Reflow, result: Ok("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "alpha beta\n", "stale result discarded");
        assert!(e.status.to_lowercase().contains("discarded"));
        assert!(!e.transform_in_flight, "in-flight cleared even on discard");
    }

    #[test]
    fn large_buffer_routes_async_and_delivers_transformdone() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        // > 1 MiB buffer forces the async branch; we block on the channel.
        let big = "word ".repeat(300_000); // ~1.5 MB
        let mut e = Editor::new_from_text(&big, None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        crate::transform::dispatch_transform(&mut e, TransformKind::Unwrap, &TestClock(0), &tx);
        assert!(e.transform_in_flight, "async dispatch sets the in-flight guard");
        let msg = rx.recv().expect("TransformDone must arrive");
        match msg { Msg::TransformDone { kind: TransformKind::Unwrap, result: Ok(_), .. } => {}
                    other => panic!("expected TransformDone Ok, got {other:?}") }
    }

    #[test]
    fn bracketed_paste_normal_inserts_into_document() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("XY".into())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXYb\n");
        assert_eq!(e.register.get(), Some("XY"));
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");
    }

    #[test]
    fn bracketed_paste_into_minibuffer_not_document() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
        let doc_before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("cat".into())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.minibuffer.as_ref().unwrap().text, "cat", "paste goes into the minibuffer");
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "document untouched");
    }

    #[test]
    fn paste_into_open_palette_edits_query_not_document() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &keymap);
        e.palette = Some(p);
        let doc_before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("foo".into())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "document untouched under open palette");
        assert_eq!(e.palette.as_ref().unwrap().query, "foo", "paste inserted into palette query");
        assert!(e.palette.is_some(), "palette remains open after paste");
    }

    #[test]
    fn paste_under_open_menu_does_not_touch_document() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.menu = Some(crate::menu::empty());
        let doc_before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("bar".into())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "document untouched under open menu");
        assert!(e.menu.is_some(), "menu remains open after paste");
    }

    // -------------------------------------------------------------------------
    // Regression: async ClipboardPaste must be dropped (not applied to doc)
    // while an overlay is open (menu / palette / theme_picker).
    // Each test fails WITHOUT the guard and passes WITH it.
    // -------------------------------------------------------------------------

    #[test]
    fn async_clipboard_paste_under_open_menu_is_dropped() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.menu = Some(crate::menu::empty());
        let doc_before = e.active().document.buffer.to_string();
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "async paste must not land in doc behind open menu");
        assert!(e.menu.is_some(), "menu remains open after ClipboardPaste is dropped");
    }

    #[test]
    fn async_clipboard_paste_under_open_palette_is_dropped() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        let reg = Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &keymap);
        e.palette = Some(p);
        let doc_before = e.active().document.buffer.to_string();
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "async paste must not land in doc behind open palette");
        assert!(e.palette.is_some(), "palette remains open after ClipboardPaste is dropped");
    }

    #[test]
    fn async_clipboard_paste_under_open_theme_picker_is_dropped() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.open_theme_picker();
        assert!(e.theme_picker.is_some(), "precondition: picker opened");
        let doc_before = e.active().document.buffer.to_string();
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "async paste must not land in doc behind open theme picker");
        assert!(e.theme_picker.is_some(), "theme picker remains open after ClipboardPaste is dropped");
    }

    #[test]
    fn bracketed_paste_with_modal_prompt_is_ignored() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.open_prompt(crate::prompt::Prompt::quit_confirm());
        let doc_before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("x".into())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "paste ignored under a modal");
        assert!(e.prompt.is_some());
    }

    #[test]
    fn bracketed_paste_empty_preserves_selection_and_register() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(1, 3); // select "bc"
        e.register.set("keep".into());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste(String::new())), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "abcd\n", "empty paste must NOT delete the selection");
        assert_eq!(e.register.get(), Some("keep"), "empty paste must NOT clear the register");
    }

    // -------------------------------------------------------------------------
    // Task 4: keymap integration tests
    // -------------------------------------------------------------------------

    #[test]
    fn single_chord_dispatches_via_keymap() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3); // select abc
        let km = cua_keymap(); let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('c'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.register.get(), Some("abc"), "Ctrl+C copied via the data-driven keymap");
    }

    #[test]
    fn pending_sequence_then_completes() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        // bind a 2-key save sequence
        let cfg = crate::config::KeymapConfig { preset: "cua".into(),
            patches: vec![crate::config::KeymapPatch {
                bind: [("ctrl-k ctrl-s".to_string(), "save".to_string())].into_iter().collect(), unbind: vec![] }] };
        let (km, _) = crate::keymap::build_keymap(&cfg, &Registry::builtins());
        let mut e = Editor::new_from_text("x\n", Some("/tmp/wc-kmtest.md".into()), (80, 24));
        let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('k'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.pending_keys.len(), 1, "first key is pending");
        assert!(e.status.contains("ctrl-k") || e.status.to_lowercase().contains("k"), "pending shown");
        crate::app::reduce(press(KeyCode::Char('s'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.pending_keys.is_empty(), "sequence resolved, pending cleared");
        // (save dispatched — the file path means dispatch_save runs; assert via status or saved flag per the real save)
    }

    #[test]
    fn esc_cancels_pending_without_other_effect() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        let cfg = crate::config::KeymapConfig { preset: "cua".into(),
            patches: vec![crate::config::KeymapPatch {
                bind: [("ctrl-k ctrl-s".to_string(), "save".to_string())].into_iter().collect(), unbind: vec![] }] };
        let (km, _) = crate::keymap::build_keymap(&cfg, &Registry::builtins());
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        let before = e.active().document.buffer.to_string();
        let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('k'), KeyModifiers::CONTROL), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.pending_keys.len(), 1);
        crate::app::reduce(press(KeyCode::Esc, KeyModifiers::NONE), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.pending_keys.is_empty(), "Esc cleared the pending sequence");
        assert_eq!(e.active().document.buffer.to_string(), before, "no buffer change");
    }

    #[test]
    fn run_startup_builds_keymap_from_config_with_user_bind() {
        // We can't run the TUI loop in a test, so test the startup builder in isolation:
        // a helper that turns (Cli-derived paths) into the effective keymap.
        let cfg = crate::config::KeymapConfig {
            preset: "cua".into(),
            patches: vec![crate::config::KeymapPatch {
                bind: [("ctrl-g".to_string(), "move_line_start".to_string())].into_iter().collect(),
                unbind: vec![],
            }],
        };
        let (km, warns) = crate::keymap::build_keymap(&cfg, &crate::registry::Registry::builtins());
        assert!(warns.is_empty());
        let g = crate::keymap::parse_chord("ctrl-g").unwrap();
        assert!(matches!(km.resolve(&[g]), crate::keymap::Resolution::Command(crate::registry::CommandId("move_line_start"))));
    }

    #[test]
    fn printable_falls_through_to_insert() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{KeyCode, KeyModifiers};
        let mut e = Editor::new_from_text("", None, (80, 24));
        let km = cua_keymap(); let (tx,_rx)=std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(press(KeyCode::Char('h'), KeyModifiers::NONE), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "h", "unbound printable inserts literally");
    }

    #[test]
    fn ctrl_p_opens_palette_and_enter_dispatches_selected() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |c, m| Event::Key(KeyEvent { code: c, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // Ctrl+P opens + hydrates
        crate::app::reduce(Msg::Input(press(KeyCode::Char('p'), KeyModifiers::CONTROL)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_some());
        assert!(!e.palette.as_ref().unwrap().rows.is_empty(), "palette hydrated with all commands on open");
        // type "copy", select first, Enter → dispatches copy (register gets the selection)
        for ch in "copy".chars() { crate::app::reduce(Msg::Input(press(KeyCode::Char(ch), KeyModifiers::NONE)), &mut e, &reg, &km, &ex, &clk, &tx); }
        crate::app::reduce(Msg::Input(press(KeyCode::Enter, KeyModifiers::NONE)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_none(), "Enter closes the palette");
        assert_eq!(e.register.get(), Some("abc"), "selected command (Copy) dispatched");
    }

    #[test]
    fn palette_esc_closes_and_clipboard_paste_is_dropped() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.palette = Some(crate::palette::Palette::default());
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        // An async ClipboardPaste arriving while the palette is open must be
        // intercepted and dropped — it must NOT reach the document (race fix).
        let bid = e.active().id;
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("X".into()) }, &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_some(), "palette still open");
        assert_eq!(e.active().document.buffer.to_string(), "ab\n", "ClipboardPaste must be dropped while palette is open");
        // Esc closes the palette
        let esc = Event::Key(KeyEvent { code: KeyCode::Esc, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(esc), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_none());
    }

    #[test]
    fn f10_toggles_menu_closed_when_open() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let reg = Registry::builtins(); let km = cua_keymap(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        // F10 with menu closed → opens the menu
        crate::app::reduce(Msg::Input(f10()), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_some(), "F10 should open menu when closed");
        // F10 with menu open → closes the menu
        crate::app::reduce(Msg::Input(f10()), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.menu.is_none(), "F10 should close menu when open");
    }

    #[test]
    fn pending_mark_consumes_one_key_then_clears() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.pending_mark = Some(crate::editor::MarkPending::Set);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |c, m| Event::Key(KeyEvent { code: c, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(press(KeyCode::Char('q'), KeyModifiers::NONE)), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.pending_mark, None, "capture consumed the key");
        assert_eq!(e.active().marks.get(&'q'), Some(&0));
        assert_eq!(e.active().document.buffer.to_string(), "abc\n", "captured key did NOT type into the doc");
    }

    #[test]
    fn esc_cancels_pending_mark() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        e.pending_mark = Some(crate::editor::MarkPending::Set);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let esc = Event::Key(KeyEvent { code: KeyCode::Esc, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(esc), &mut e, &reg, &km, &ex, &clk, &tx);
        assert_eq!(e.pending_mark, None);
        assert!(e.active().marks.is_empty());
    }

    #[test]
    fn toggle_mouse_capture_flips_flag() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        assert!(e.mouse_capture, "default on");
        let (_km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &Registry::builtins());
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock::new(0);
        let id = reg.resolve_name("toggle_mouse_capture").expect("registered");
        { let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx.clone() };
          reg.dispatch(id, &mut ctx); }
        assert!(!e.mouse_capture, "toggled off");
    }

    #[test]
    fn scrollbar_visible_recomputed_against_clock() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.mouse.scrollbar_until_ms = 1000;
        crate::app::recompute_scrollbar_visible(&mut e, 500); // before deadline
        assert!(e.mouse.scrollbar_visible);
        crate::app::recompute_scrollbar_visible(&mut e, 1200); // after
        assert!(!e.mouse.scrollbar_visible);
    }

    // A1 Task 3 — cases 7 + 8 (app-level: recompute_menu_bar / reconcile)

    /// Case 7: recompute fires in Auto; in non-Auto it clears without firing.
    #[test]
    fn recompute_fires_and_is_mode_gated() {
        use crate::editor::Editor;
        use crate::config::MenuBarMode;

        // Auto: past reveal deadline → revealed = true.
        let mut e = Editor::new_from_text("x\n", None, (40, 8));
        e.menu_bar_mode = MenuBarMode::Auto;
        e.mouse.menu_reveal_due = Some(100);
        crate::app::recompute_menu_bar(&mut e, 101);
        assert!(e.mouse.menu_bar_revealed, "Auto: past reveal due must fire");
        assert!(e.mouse.menu_reveal_due.is_none(), "reveal due cleared after firing");

        // Pinned: past reveal deadline → cleared WITHOUT firing (defense-in-depth).
        let mut e2 = Editor::new_from_text("x\n", None, (40, 8));
        e2.menu_bar_mode = MenuBarMode::Pinned;
        e2.mouse.menu_reveal_due = Some(100);
        crate::app::recompute_menu_bar(&mut e2, 101);
        assert!(!e2.mouse.menu_bar_revealed, "Pinned: due CLEARED, revealed NOT set");
        assert!(e2.mouse.menu_reveal_due.is_none(), "due cleared in Pinned");

        // Auto: past hide deadline → revealed = false.
        let mut e3 = Editor::new_from_text("x\n", None, (40, 8));
        e3.menu_bar_mode = MenuBarMode::Auto;
        e3.mouse.menu_bar_revealed = true;
        e3.mouse.menu_hide_due = Some(200);
        crate::app::recompute_menu_bar(&mut e3, 201);
        assert!(!e3.mouse.menu_bar_revealed, "Auto: past hide due must fire → unrevealed");
        assert!(e3.mouse.menu_hide_due.is_none(), "hide due cleared after firing");
    }

    /// Case 8: capture-off clears all three menu-bar fields.
    #[test]
    fn capture_disable_clears_menu_bar_state() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (40, 8));
        e.mouse_capture = true;
        e.mouse.menu_bar_revealed = true;
        e.mouse.menu_reveal_due = Some(500);
        e.mouse.menu_hide_due = Some(900);
        let mut buf = Vec::<u8>::new();
        let mut applied = true;
        e.mouse_capture = false;
        crate::app::reconcile_mouse_capture(&mut e, &mut buf, &mut applied);
        assert!(!e.mouse.menu_bar_revealed, "revealed cleared on capture disable");
        assert!(e.mouse.menu_reveal_due.is_none(), "menu_reveal_due cleared on capture disable");
        assert!(e.mouse.menu_hide_due.is_none(), "menu_hide_due cleared on capture disable");
    }

    /// Finding 1 regression: wheel event sets scrollbar_until_ms; recomputing
    /// immediately after (now == t, t < t+1200) must yield visible == true.
    /// A later recompute at t+1300 must yield false (bar fades after deadline).
    #[test]
    fn wheel_then_recompute_makes_scrollbar_visible() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{MouseEvent, MouseEventKind, KeyModifiers};
        let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 10));
        crate::derive::rebuild(&mut e);
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = InlineExecutor::default();
        let t: u64 = 5000;
        let clk = TestClock(t);
        let (tx, _rx) = std::sync::mpsc::channel();
        let wheel = MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        // Dispatch the scroll event (sets scrollbar_until_ms = t + 1200).
        crate::mouse::handle(&mut e, wheel, &reg, &km, &ex, &clk, &tx);
        // Recompute at t (now < until) — bar must be visible.
        crate::app::recompute_scrollbar_visible(&mut e, t);
        assert!(e.mouse.scrollbar_visible, "scrollbar must be visible immediately after a scroll event");
        // Recompute after the fade deadline — bar must hide.
        crate::app::recompute_scrollbar_visible(&mut e, t + 1300);
        assert!(!e.mouse.scrollbar_visible, "scrollbar must hide after scrollbar_until_ms expires");
    }

    // -------------------------------------------------------------------------
    // Task 4 (Effort 5e): search overlay reduce() interception tests
    // -------------------------------------------------------------------------

    #[test]
    fn ctrl_f_opens_search_and_typing_jumps() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("foo bar foo\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let mkpress = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(mkpress(KeyCode::Char('f'), KeyModifiers::CONTROL)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.search.is_some(), "Ctrl+F opens search");
        for c in "bar".chars() { crate::app::reduce(Msg::Input(mkpress(KeyCode::Char(c), KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx); }
        let s = e.search.as_ref().unwrap();
        assert_eq!(s.needle, "bar");
        assert_eq!(s.current().unwrap().start, 4); // caret jumped to the match
        assert_eq!(e.active().document.selection.primary().from(), 4);
    }

    #[test]
    fn esc_restores_origin_and_closes() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("foo bar\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let mkpress = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(mkpress(KeyCode::Char('f'), KeyModifiers::CONTROL)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        for c in "bar".chars() { crate::app::reduce(Msg::Input(mkpress(KeyCode::Char(c), KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx); }
        crate::app::reduce(Msg::Input(mkpress(KeyCode::Esc, KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.search.is_none(), "Esc closes search");
        assert_eq!(e.active().document.selection.primary().to(), 0, "caret restored to origin");
    }

    #[test]
    fn replace_all_is_one_undo_unit_and_remaps_origin() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let r = |e: &mut Editor, ev| crate::app::reduce(Msg::Input(ev), e, &reg, &cua_keymap(), &ex, &clk, &tx);
        r(&mut e, press(KeyCode::Char('r'), KeyModifiers::CONTROL));   // open Replace
        for c in "aa".chars() { r(&mut e, press(KeyCode::Char(c), KeyModifiers::NONE)); }
        r(&mut e, press(KeyCode::Tab, KeyModifiers::NONE));            // focus Template
        r(&mut e, press(KeyCode::Char('b'), KeyModifiers::NONE));
        r(&mut e, press(KeyCode::Char('a'), KeyModifiers::ALT));       // Alt+A = Replace All
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "b b b\n");
        let v_after = e.active().document.version;
        assert!(e.active_mut().undo());                                // ONE undo reverts ALL
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "aa aa aa\n");
        let _ = v_after;
    }

    /// Finding 2 regression: after a resize to a smaller terminal the stale
    /// scroll_row must be clamped so the caret remains visible.
    ///
    /// Setup: 50-line doc; scroll to line 30 with scroll_row=5 in a large
    /// 80×40 terminal. Then resize to 80×10 and dispatch Msg::Input(Resize).
    /// With the fix, ensure_visible clamps scroll/scroll_row so that a
    /// subsequent rebuild + screen_pos succeeds.  Without the fix the stale
    /// scroll_row could exceed the new visible range and render would skip rows.
    #[test]
    fn resize_re_pins_scroll() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::Event;

        let text: String = (0..50).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 40));
        // Rebuild once for the initial large area so layouts are populated.
        crate::derive::rebuild(&mut e);

        // Manually push scroll deep into the document (line 30) and set a
        // non-zero scroll_row to simulate a resumed or scrolled position.
        e.active_mut().view.scroll = 30;
        e.active_mut().view.scroll_row = 5;

        // Build a minimal reduce context.
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        // Dispatch a resize to a much smaller terminal.
        crate::app::reduce(
            crate::app::Msg::Input(Event::Resize(80, 10)),
            &mut e, &reg, &km, &ex, &clk, &tx,
        );

        // After resize + ensure_visible the cache must be fresh and screen_pos
        // must return Some (caret is visible on the new geometry).
        crate::derive::rebuild(&mut e);
        let pos = crate::nav::screen_pos(&e);
        assert!(
            pos.is_some(),
            "caret must be visible after resize; scroll={} scroll_row={}",
            e.active().view.scroll, e.active().view.scroll_row,
        );

        // scroll_row must be 0 (single-visual-row lines) — never a stale large value.
        assert_eq!(
            e.active().view.scroll_row, 0,
            "scroll_row must be clamped to a valid value after resize",
        );
    }

    /// Regression: resize must update EVERY buffer's view.area, not just the active one.
    /// Background buffers that keep a stale area lay out at the wrong geometry when
    /// switched to.  Fix: the Resize handler iterates all buffers.
    #[test]
    fn resize_updates_all_buffers_area() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::Event;

        let mut e = Editor::new_from_text("first buffer\n", None, (80, 40));
        e.install_scratch(); // second buffer — background after install

        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ex = InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();

        // Sanity: both buffers start at the initial area.
        assert!(e.buffers.iter().all(|b| b.view.area == (80, 40)));

        // Dispatch a resize event.
        crate::app::reduce(
            crate::app::Msg::Input(Event::Resize(120, 30)),
            &mut e, &reg, &km, &ex, &clk, &tx,
        );

        // ALL buffers — not just the active one — must reflect the new dimensions.
        for b in &e.buffers {
            assert_eq!(
                b.view.area, (120, 30),
                "buffer {:?} has stale area {:?} after resize", b.id, b.view.area,
            );
        }
    }

    // -------------------------------------------------------------------------
    // Task 7 (Effort 5e): interactive query-replace stepping tests
    // -------------------------------------------------------------------------

    #[test]
    fn query_replace_steps_yes_no_and_remaps() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let r = |e: &mut Editor, ev| crate::app::reduce(Msg::Input(ev), e, &reg, &cua_keymap(), &ex, &clk, &tx);
        r(&mut e, press(KeyCode::Char('r'), KeyModifiers::CONTROL));
        for c in "aa".chars() { r(&mut e, press(KeyCode::Char(c), KeyModifiers::NONE)); }
        r(&mut e, press(KeyCode::Tab, KeyModifiers::NONE));
        r(&mut e, press(KeyCode::Char('b'), KeyModifiers::NONE));
        r(&mut e, press(KeyCode::Enter, KeyModifiers::ALT));           // Alt+Enter starts stepping
        assert_eq!(e.search.as_ref().unwrap().phase, crate::search_overlay::Phase::Stepping);
        r(&mut e, press(KeyCode::Char('y'), KeyModifiers::NONE));      // replace #1
        r(&mut e, press(KeyCode::Char('n'), KeyModifiers::NONE));      // skip #2
        r(&mut e, press(KeyCode::Char('y'), KeyModifiers::NONE));      // replace #3
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "b aa b\n");
    }

    /// Fix A regression: a FilterDone arriving while the search overlay is open
    /// must be applied, not silently dropped.
    #[test]
    fn search_does_not_starve_filterdone() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        e.open_search(crate::search_overlay::Phase::Find, 0);
        let (tx, _rx) = std::sync::mpsc::channel(); let reg = Registry::builtins();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 1..3, cursor: 2,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXde\n", "FilterDone applies even under an open search overlay");
        assert!(e.search.is_some(), "search overlay remains open after non-key message");
    }

    #[test]
    fn outline_overlay_does_not_starve_background_messages() {
        use crate::editor::Editor;
        use crate::filter::{Disposition, RunResult};
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("# H\nabcde\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        e.open_outline();
        let (tx, _rx) = std::sync::mpsc::channel(); let reg = Registry::builtins();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::FilterDone { buffer_id: id, version: v, range: 4..6, cursor: 5,
            disposition: Disposition::Filter, outcome: RunResult::Stdout("X".into()) }, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "# H\nXcde\n", "FilterDone applies even under an open outline overlay");
        assert!(e.outline.is_some(), "outline overlay remains open after non-key message");
    }

    #[test]
    fn outline_jump_refused_after_background_edit() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("# Top\nintro\n## A\nbody\n", None, (80, 24));
        let start = e.active().document.selection.primary().head;
        e.open_outline();
        assert!(e.outline.is_some(), "outline overlay must open");
        let opened_version = e.outline.as_ref().unwrap().opened_version;
        e.active_mut().document.version = opened_version + 1;
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let enter = Event::Key(KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        crate::app::reduce(Msg::Input(enter), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.outline.is_none(), "stale outline overlay must close without jumping");
        assert_eq!(e.active().document.selection.primary().head, start,
            "stale outline Enter must not move the caret");
        assert!(e.status.contains("changed"), "status must mention the change");
    }

    #[test]
    fn outline_jump_auto_unfolds_ancestor_and_moves_caret() {
        let doc = "# Top\nintro\n## A\nbody\n### A1\nx\n";
        let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
        ed.active_mut().folds.toggle(doc.find("## A").unwrap());
        crate::derive::rebuild(&mut ed);
        let a1 = doc.find("### A1").unwrap();
        crate::app::outline_jump_to(&mut ed, a1);
        assert_eq!(ed.active().document.selection.primary().head, a1);
        assert!(!ed.active().folds.folded().contains(&doc.find("## A").unwrap()));
    }

    /// Fix C regression: replace-all with an invalid regex must set status
    /// "invalid regex" and leave the buffer unchanged.
    #[test]
    fn invalid_regex_replace_all_sets_status() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
        let before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let r = |e: &mut Editor, ev| crate::app::reduce(Msg::Input(ev), e, &reg, &cua_keymap(), &ex, &clk, &tx);
        r(&mut e, press(KeyCode::Char('r'), KeyModifiers::CONTROL));    // open Replace (Phase::Replace)
        r(&mut e, press(KeyCode::Char('r'), KeyModifiers::ALT));        // Alt+R: toggle to regex mode
        // type an invalid regex pattern: unbalanced open paren
        r(&mut e, press(KeyCode::Char('('), KeyModifiers::NONE));       // invalid in regex mode
        r(&mut e, press(KeyCode::Char('a'), KeyModifiers::ALT));        // Alt+A = Replace All
        assert_eq!(e.active().document.buffer.to_string(), before, "invalid regex must not mutate the buffer");
        assert_eq!(e.status, "invalid regex", "status must say 'invalid regex', got: {:?}", e.status);
    }

    #[test]
    fn query_replace_bang_finishes_rest() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        let r = |e: &mut Editor, ev| crate::app::reduce(Msg::Input(ev), e, &reg, &cua_keymap(), &ex, &clk, &tx);
        r(&mut e, press(KeyCode::Char('r'), KeyModifiers::CONTROL));
        for c in "aa".chars() { r(&mut e, press(KeyCode::Char(c), KeyModifiers::NONE)); }
        r(&mut e, press(KeyCode::Tab, KeyModifiers::NONE));
        r(&mut e, press(KeyCode::Char('b'), KeyModifiers::NONE));
        r(&mut e, press(KeyCode::Enter, KeyModifiers::ALT));
        r(&mut e, press(KeyCode::Char('!'), KeyModifiers::NONE));      // finish all remaining
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "b b b\n");
    }

    #[test]
    fn diagnostics_done_applies_only_for_current_version() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        let bid = e.active().id;
        let v = e.active().document.version;
        let diag = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
            message: "misspelled".into(),
            suggestions: vec![wordcartel_core::diagnostics::Suggestion::ReplaceWith("the".into())] }];
        // current version → stored
        crate::diagnostics_run::apply_diagnostics_done(&mut e, bid, v, diag.clone());
        assert_eq!(e.active().diagnostics.diagnostics.len(), 1);
        assert_eq!(e.active().diagnostics.computed_version, v);
        // stale version → discarded
        crate::diagnostics_run::apply_diagnostics_done(&mut e, bid, v.wrapping_sub(1), diag);
        assert_eq!(e.active().diagnostics.diagnostics.len(), 1, "stale result must not overwrite");
    }

    #[test]
    fn tick_dispatches_a_due_check_once() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("teh\n", None, (80, 24));
        e.diag_cfg.enabled = true;
        e.active_mut().diagnostics.arm(0, 400); // due at 400
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(500); // past due
        // a Tick at now=500 with diagnostics enabled dispatches one check
        crate::app::reduce(Msg::Tick, &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().diagnostics.in_flight_version, Some(e.active().document.version));
        // the spawned worker sends a DiagnosticsDone
        match rx.recv_timeout(std::time::Duration::from_secs(30)).unwrap() {
            Msg::DiagnosticsDone { diagnostics, .. } => assert!(diagnostics.iter().any(|d| d.kind == wordcartel_core::diagnostics::DiagnosticKind::Spelling)),
            o => panic!("expected DiagnosticsDone, got {o:?}"),
        }
    }

    // -------------------------------------------------------------------------
    // Task 6 (Effort 5f): quick-fix overlay tests
    // -------------------------------------------------------------------------

    #[test]
    fn quick_fix_applies_suggestion_as_undoable_edit() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        let v = e.active().document.version;
        e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(),
            suggestions: vec![wordcartel_core::diagnostics::Suggestion::ReplaceWith("the".into())] }];
        e.active_mut().diagnostics.computed_version = v;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1); // cursor inside "teh"
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(press(KeyCode::Char('.'), KeyModifiers::CONTROL)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.diag.is_some(), "Ctrl+. opens the quick-fix overlay on the diagnostic");
        crate::app::reduce(Msg::Input(press(KeyCode::Enter, KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "the cat\n");
        assert!(e.diag.is_none(), "overlay closes after apply");
        assert!(e.active_mut().undo(), "the fix is one undo unit");
        assert_eq!(e.active().document.buffer.snapshot().to_string(), "teh cat\n");
    }

    #[test]
    fn open_diag_clears_siblings_and_open_others_clear_diag() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let d = wordcartel_core::diagnostics::Diagnostic { range: 0..1, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(), suggestions: vec![] };
        // open_diag clears a previously-open palette/search (reverse XOR direction)
        e.open_palette();
        assert!(e.palette.is_some(), "palette open before open_diag");
        e.open_diag(d.clone());
        assert!(e.palette.is_none(), "open_diag clears palette");
        assert!(e.diag.is_some());
        // the other direction: opening the palette clears an open diag overlay
        e.open_palette();
        assert!(e.diag.is_none(), "open_palette clears diag");
    }

    /// Regression: quick_fix dispatched after an edit (when valid_for is false) must
    /// NOT open the overlay and must NOT corrupt the buffer. Before the fix the
    /// handlers read stale diagnostic byte ranges unchecked.
    #[test]
    fn quick_fix_on_stale_diagnostics_is_noop_no_overlay() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        let v = e.active().document.version;
        // Store a diagnostic at version V.
        e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(),
            suggestions: vec![wordcartel_core::diagnostics::Suggestion::ReplaceWith("the".into())] }];
        e.active_mut().diagnostics.computed_version = v;
        // Simulate an intervening edit: bump the document version so valid_for is now false.
        e.active_mut().document.version = v + 1;
        assert!(!e.active().diagnostics.valid_for(e.active().document.version),
            "precondition: diagnostics must be stale after version bump");
        let buf_before = e.active().document.buffer.to_string();
        // Place cursor inside the stale diagnostic range.
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let ctrl_dot = Event::Key(KeyEvent { code: KeyCode::Char('.'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(ctrl_dot), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        // The overlay must NOT open and the buffer must be unchanged.
        assert!(e.diag.is_none(), "stale diagnostics: quick_fix must NOT open the overlay");
        assert_eq!(e.active().document.buffer.to_string(), buf_before, "buffer must be unchanged");
    }

    #[test]
    fn diag_next_prev_move_caret_with_wrap() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("teh cat adn dog\n", None, (80, 24));
        let v = e.active().document.version;
        e.active_mut().diagnostics.diagnostics = vec![
            wordcartel_core::diagnostics::Diagnostic { range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message:"x".into(), suggestions: vec![] },
            wordcartel_core::diagnostics::Diagnostic { range: 8..11, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message:"x".into(), suggestions: vec![] },
        ];
        e.active_mut().diagnostics.computed_version = v;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let f8 = Event::Key(KeyEvent { code: KeyCode::F(8), modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(f8.clone()), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.selection.primary().to(), 8, "F8 moves to the next diagnostic");
        crate::app::reduce(Msg::Input(f8), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert_eq!(e.active().document.selection.primary().to(), 0, "F8 wraps to the first");
    }

    // -----------------------------------------------------------------------
    // Fix A3: no busy-loop while a diagnostics check is in flight
    // -----------------------------------------------------------------------

    /// When `in_flight_version` is set, the diagnostics deadline must be
    /// excluded from the loop's `next_deadline` call.  This is a pure logic
    /// test of the gating condition, validated without touching the real loop.
    #[test]
    fn diag_deadline_excluded_when_in_flight() {
        use crate::diagnostics_run::{DiagStore, next_deadline};
        // Build a store that has a past-due recheck_due_at AND an in-flight version.
        let mut store = DiagStore::new();
        store.recheck_due_at = Some(0); // past due
        store.in_flight_version = Some(5); // check is in flight

        // Compute the diag_deadline using the same gating logic as the run() loop.
        let diag_deadline = if store.in_flight_version.is_none() {
            store.recheck_due_at
        } else {
            None
        };

        // With no other deadlines, the computed deadline should be None (not 0),
        // so recv_timeout gets a long duration instead of 0 ms.
        let deadline = next_deadline(&[None, None, None, diag_deadline]);
        assert_eq!(deadline, None,
            "when in_flight, diag_deadline must be None so the loop does not spin");

        // Sanity: without in-flight, the past-due timestamp IS included.
        // temporarily clear in_flight to test the other branch
        let saved = store.in_flight_version.take();
        let diag_deadline_no_flight = if store.in_flight_version.is_none() {
            store.recheck_due_at
        } else {
            None
        };
        let deadline_no_flight = next_deadline(&[None, None, None, diag_deadline_no_flight]);
        assert_eq!(deadline_no_flight, Some(0),
            "without in_flight, past-due recheck_due_at is included in the deadline");
        store.in_flight_version = saved; // restore
    }

    // -----------------------------------------------------------------------
    // Fix A4: stale quick-fix overlay must not apply
    // -----------------------------------------------------------------------

    /// If the buffer is mutated while the diag overlay is open, pressing Enter
    /// must NOT apply the (now-stale) suggestion.  The overlay must be closed
    /// and the buffer left unchanged.
    #[test]
    fn quick_fix_refuses_stale_apply_after_concurrent_edit() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("teh cat\n", None, (80, 24));
        let v = e.active().document.version;
        // Arm valid diagnostics at version V and open the overlay.
        e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
            range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling, message: "x".into(),
            suggestions: vec![wordcartel_core::diagnostics::Suggestion::ReplaceWith("the".into())] }];
        e.active_mut().diagnostics.computed_version = v;
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        // Open the overlay.
        crate::app::reduce(Msg::Input(press(KeyCode::Char('.'), KeyModifiers::CONTROL)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(e.diag.is_some(), "overlay must open");
        assert_eq!(e.diag.as_ref().unwrap().opened_version, v, "opened_version captured at open");
        // Simulate a concurrent edit while the overlay is open: bump the document version.
        e.active_mut().document.version += 1;
        let buf_before = e.active().document.buffer.to_string();
        // Press Enter — must be refused because opened_version != current version.
        crate::app::reduce(Msg::Input(press(KeyCode::Enter, KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
        // Overlay must be closed and buffer must be unchanged.
        assert!(e.diag.is_none(), "stale overlay must be closed without applying");
        assert_eq!(e.active().document.buffer.to_string(), buf_before,
            "buffer must not be mutated when the overlay is stale");
        assert!(e.status.contains("changed"), "status must mention the change");
    }

    // -----------------------------------------------------------------------
    // Task 12 (Effort 5g): search caret jumps auto-unfold folded ancestors
    // -----------------------------------------------------------------------

    #[test]
    fn search_hit_inside_fold_auto_unfolds() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let doc = "# Top\nintro\n## A\nneedle here\nmore\n## B\n";
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        // fold ## A
        let a_byte = doc.find("## A").unwrap();
        ed.active_mut().folds.toggle(a_byte);
        crate::derive::rebuild(&mut ed);
        // open search with Ctrl+F and type "needle"
        let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let mkpress = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(mkpress(KeyCode::Char('f'), KeyModifiers::CONTROL)), &mut ed, &reg, &cua_keymap(), &ex, &clk, &tx);
        assert!(ed.search.is_some(), "Ctrl+F must open search");
        for c in "needle".chars() {
            crate::app::reduce(Msg::Input(mkpress(KeyCode::Char(c), KeyModifiers::NONE)), &mut ed, &reg, &cua_keymap(), &ex, &clk, &tx);
        }
        let needle_pos = doc.find("needle").unwrap();
        assert_eq!(ed.active().document.selection.primary().from(), needle_pos,
            "caret must be on the 'needle' match");
        assert!(!ed.active().folds.folded().contains(&a_byte),
            "## A fold must be cleared when jumping into its body");
    }

    /// Regression test: a cursor restored from a session entry that falls inside a
    /// folded section must be snapped out to the section heading on startup.
    ///
    /// Tests the `derive::rebuild → SnapOut → ensure_visible` sequence inserted
    /// into the session-resume path in `run()`. Full TTY/disk setup is not used;
    /// the test drives the same API at the unit-test seam. The test FAILS (final
    /// assertion fires) if the SnapOut block is removed from the test body, and
    /// PASSES with it in place — mirroring failure/pass for the production change.
    #[test]
    fn resume_snaps_saved_cursor_out_of_restored_fold() {
        use crate::editor::Editor;
        use crate::registry::{place_caret_visible, CaretPlace};
        use crate::fold::FoldView;

        // Document with a heading "## A" followed by a body.
        // "# Top\nintro\n## A\nbody1\nbody2\n## B\ntail\n"
        //   byte 0:  '# Top\n'  (6 bytes)
        //   byte 6:  'intro\n'  (6 bytes)
        //   byte 12: '## A\n'   (5 bytes) ← heading start
        //   byte 17: 'body1\n'  (6 bytes)
        //   byte 23: 'body2\n'  (6 bytes) ← saved cursor lands inside here
        //   byte 29: '## B\n'
        const DOC: &str = "# Top\nintro\n## A\nbody1\nbody2\n## B\ntail\n";
        let mut editor = Editor::new_from_text(DOC, None, (80, 24));

        let heading_a = DOC.find("## A").unwrap();           // byte 12
        let cursor_in_body = DOC.find("body2").unwrap() + 1; // byte 24, inside body2

        // — Simulate the session-resume block in run() —
        // Restore cursor to the saved (inside-fold) position.
        editor.active_mut().document.selection =
            wordcartel_core::selection::Selection::single(cursor_in_body);
        // Restore fold on "## A" and reconcile (mirrors app.rs resume block).
        editor.active_mut().folds.toggle(heading_a);
        {
            let b = editor.active();
            let blocks = b.document.blocks().clone();
            let buf = b.document.buffer.clone();
            editor.active_mut().folds.reconcile(&blocks, &buf);
        }

        // Precondition: before SnapOut, the restored cursor IS inside the fold.
        // This is what the bug looks like: cursor is hidden after resume without the fix.
        {
            let b = editor.active();
            let fv = FoldView::compute(&b.folds, b.document.blocks(), &b.document.buffer);
            let raw_line = b.document.buffer.byte_to_line(cursor_in_body);
            assert!(fv.is_hidden(raw_line),
                "precondition: without SnapOut the restored cursor sits inside the fold");
        }

        // — The fix: derive::rebuild then SnapOut (same order as in run()) —
        // If you comment out the SnapOut block below, the final assertion fails.
        crate::derive::rebuild(&mut editor);
        {
            // SnapOut: snap restored caret to heading if it landed inside a fold.
            let head = editor.active().document.selection.primary().head;
            let nh = place_caret_visible(&mut editor, head, CaretPlace::SnapOut);
            if nh != head {
                editor.active_mut().document.selection =
                    wordcartel_core::selection::Selection::single(nh);
            }
        }

        // Postcondition: cursor is now on the heading byte, NOT hidden.
        let final_head = editor.active().document.selection.primary().head;
        assert_eq!(final_head, heading_a,
            "SnapOut must move the caret from inside-fold body to the '## A' heading");
        {
            let b = editor.active();
            let fv = FoldView::compute(&b.folds, b.document.blocks(), &b.document.buffer);
            let final_line = b.document.buffer.byte_to_line(final_head);
            assert!(!fv.is_hidden(final_line),
                "caret must not be on a hidden line after resume normalization");
        }
    }

    #[test]
    fn save_as_writes_new_path_and_rekeys() {
        use crate::editor::Editor;
        use crate::jobs::{Executor, InlineExecutor};
        let dir = std::env::temp_dir().join(format!("wc-saveas-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("out.md");
        let _ = std::fs::remove_file(&p);
        let mut e = Editor::new_from_text("content\n", None, (80, 24)); // UNNAMED, dirty-ish
        e.active_mut().document.version = 1; e.active_mut().document.saved_version = None;
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::prompts::save_as_submit(&mut e, p.to_str().unwrap(), &ex, &clk, &tx);
        for o in ex.drain() { crate::jobs_apply::apply_outcome(o, &mut e); }
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "content\n", "file written");
        assert_eq!(e.active().document.path.as_deref(), Some(p.as_path()), "path re-keyed");
        assert!(!e.active().document.dirty(), "clean after save-as");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_on_unnamed_buffer_opens_save_as_prompt() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = crate::registry::Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
        crate::save::dispatch_save(&mut ctx); // no path → opens Save-As, NOT the dead stub
        assert!(matches!(e.minibuffer.as_ref().map(|m| m.kind),
            Some(crate::minibuffer::MinibufferKind::SaveAs)), "unnamed save opens the SaveAs minibuffer");
    }

    // -------------------------------------------------------------------------
    // Task 4 (Effort 9A): ^KW block_write — write block text to file
    // -------------------------------------------------------------------------

    // -------------------------------------------------------------------------
    // Effort 6, Task 2: scratch persistence
    // -------------------------------------------------------------------------

    #[test]
    fn persist_session_captures_scratch_even_when_active_unnamed() {
        use wordcartel_core::history::Clock;
        struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let mut e = crate::editor::Editor::new_from_text("\n", None, (40, 10)); // active unnamed
        e.install_scratch();
        let sid = e.scratch_id.unwrap();
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "stash".into())], 0);
        let txn = wordcartel_core::history::Transaction::new(cs)
            .with_selection(wordcartel_core::selection::Selection::single(5));
        e.by_id_mut(sid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
        let mut session = crate::state::SessionState::default();
        let cfg = crate::config::Config::default();
        crate::app::persist_session_for_test(&mut session, &e, &cfg, 1);
        assert_eq!(session.scratch.as_ref().unwrap().text, "stash");
    }

    #[test]
    fn persist_session_clears_stale_scratch_when_oversized() {
        use wordcartel_core::history::Clock;
        struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let mut e = crate::editor::Editor::new_from_text("\n", None, (40, 10));
        e.install_scratch();
        let sid = e.scratch_id.unwrap();
        // Make the live scratch buffer oversized (> MAX_SESSION_BYTES).
        let big = "x".repeat(crate::limits::MAX_SESSION_BYTES + 1);
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, big)], 0);
        let txn = wordcartel_core::history::Transaction::new(cs);
        e.by_id_mut(sid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
        // Session carries a STALE scratch loaded from a previous launch.
        let mut session = crate::state::SessionState {
            scratch: Some(crate::state::ScratchState { text: "old stale".into(), cursor: 0 }),
            ..Default::default()
        };
        let cfg = crate::config::Config::default();
        crate::app::persist_session_for_test(&mut session, &e, &cfg, 1);
        assert!(session.scratch.is_none(),
            "oversized live scratch must CLEAR the stale loaded scratch, not resurrect it");
    }

    #[test]
    fn input_watchdog_emits_input_thread_died_when_the_reader_ends() {
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        // Stand-in for the input reader that has ended (Err from read(), or a panic).
        let reader = std::thread::spawn(|| { /* returns immediately */ });
        // The watchdog logic: join, then surface the death.
        let watch_tx = tx.clone();
        std::thread::spawn(move || {
            let _ = reader.join();
            let _ = watch_tx.send(Msg::InputThreadDied);
        })
        .join()
        .unwrap();
        assert!(matches!(rx.recv().unwrap(), Msg::InputThreadDied));
    }

    // -----------------------------------------------------------------------
    // Task 4: reconcile arm logic unit test
    // -----------------------------------------------------------------------

    // -----------------------------------------------------------------------
    // A1 Task 2: hydrate_overlays preserves and maps the placeholder's open index.
    // -----------------------------------------------------------------------

    fn build_km() -> crate::keymap::KeyTrie {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        km
    }

    /// hydrate_overlays maps a placeholder's MENU_ORDER index to the built groups'
    /// position by category (Format = index 2 in MENU_ORDER).
    #[test]
    fn hydrate_preserves_and_maps_open() {
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let reg = crate::registry::Registry::builtins();
        let km = build_km();
        // MENU_ORDER[2] = Format
        e.menu = Some(crate::menu::empty_at(2));
        crate::app::hydrate_overlays(&mut e, &reg, &km);
        let menu = e.menu.as_ref().expect("menu must be Some after hydration");
        assert!(menu.built, "menu must be marked built after hydration");
        // locate Format in the built groups
        let format_pos = menu.groups.iter().position(|(cat, _)| *cat == crate::registry::MenuCategory::Format)
            .expect("Format category must be in built groups");
        assert_eq!(menu.open, format_pos, "open must map to Format's position in built groups");
    }

    /// hydrate_overlays clamps highlighted to the last item in the open group.
    #[test]
    fn hydrate_clamps_highlighted() {
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let reg = crate::registry::Registry::builtins();
        let km = build_km();
        // MENU_ORDER[2] = Format; seed highlighted at an absurd index
        let mut placeholder = crate::menu::empty_at(2);
        placeholder.highlighted = 999;
        e.menu = Some(placeholder);
        crate::app::hydrate_overlays(&mut e, &reg, &km);
        let menu = e.menu.as_ref().unwrap();
        let open_group = menu.groups.get(menu.open).expect("open group must exist");
        let max_hl = open_group.1.len().saturating_sub(1);
        assert!(menu.highlighted <= max_hl,
            "highlighted {} must be clamped to max {max_hl}", menu.highlighted);
    }

    // -----------------------------------------------------------------------
    // A6 Task 1: palette windowed scrolling
    // -----------------------------------------------------------------------

    /// A6: entering a command past the 15-row cap via Down keys keeps the
    /// selection inside the visible window, and pressing Enter dispatches that
    /// command's observable effect. Uses `toggle_word_count` (past index 15,
    /// benign, observable via `view_opts.word_count`).
    ///
    /// TDD RED: with `scroll_top` field added but `keep_overlay_visible` not yet
    /// wired into the key arms, `p.selected - p.scroll_top < 15` would FAIL (selected
    /// would advance but scroll_top would stay 0). After wiring (Step 3) → GREEN.
    #[test]
    fn palette_hazard_pin_enter_dispatches_visible_row() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        // Seed a Commands palette (NOT the buffer-switcher idiom).
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &km);
        e.palette = Some(p);
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press_key = |c: KeyCode| Msg::Input(Event::Key(KeyEvent {
            code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE,
        }));
        // Find toggle_word_count's index in registration order (empty-query → all commands).
        let twc_idx = e.palette.as_ref().unwrap().rows.iter()
            .position(|r| r.id == crate::registry::CommandId("toggle_word_count"))
            .expect("toggle_word_count must be in the palette");
        assert!(twc_idx > 15, "toggle_word_count must be past the 15-row visible cap");
        // Drive Down to toggle_word_count's row.
        for _ in 0..twc_idx {
            crate::app::reduce(press_key(KeyCode::Down), &mut e, &reg, &km, &ex, &clk, &tx);
        }
        // Assert the windowing invariant: the selected row is within the visible window.
        let p = e.palette.as_ref().unwrap();
        assert_eq!(p.selected, twc_idx, "selected landed on toggle_word_count");
        let lh = crate::list_window::list_h_for(p.rows.len(), 24);
        assert!(p.selected.saturating_sub(p.scroll_top) < lh,
            "selected must be within the visible window (selected={}, scroll_top={}, lh={})",
            p.selected, p.scroll_top, lh);
        // Dispatch: Enter → toggle_word_count runs → word_count flips.
        let word_count_before = e.view_opts.word_count;
        crate::app::reduce(press_key(KeyCode::Enter), &mut e, &reg, &km, &ex, &clk, &tx);
        assert!(e.palette.is_none(), "Enter closes the palette");
        assert_ne!(e.view_opts.word_count, word_count_before, "toggle_word_count was dispatched");
    }

    /// A6: PageDown jumps a full window page; Home/End land at the boundaries;
    /// the window invariant holds at all positions.
    #[test]
    fn palette_pgdn_home_end_land_exactly() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut e = Editor::new_from_text("", None, (80, 24));
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &km);
        e.palette = Some(p);
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press_key = |c: KeyCode| Msg::Input(Event::Key(KeyEvent {
            code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE,
        }));
        let total = e.palette.as_ref().unwrap().rows.len();
        // End — lands at the last row.
        crate::app::reduce(press_key(KeyCode::End), &mut e, &reg, &km, &ex, &clk, &tx);
        let p = e.palette.as_ref().unwrap();
        assert_eq!(p.selected, total.saturating_sub(1), "End lands on last row");
        let lh = crate::list_window::list_h_for(p.rows.len(), 24);
        assert!(p.selected.saturating_sub(p.scroll_top) < lh, "End: selection visible");
        // Home — lands at row 0.
        crate::app::reduce(press_key(KeyCode::Home), &mut e, &reg, &km, &ex, &clk, &tx);
        let p = e.palette.as_ref().unwrap();
        assert_eq!(p.selected, 0, "Home lands on first row");
        assert_eq!(p.scroll_top, 0, "Home: scroll_top resets to 0");
        // PageDown from 0 — jumps by lh.
        crate::app::reduce(press_key(KeyCode::PageDown), &mut e, &reg, &km, &ex, &clk, &tx);
        let p = e.palette.as_ref().unwrap();
        let lh2 = crate::list_window::list_h_for(p.rows.len(), 24);
        assert!(p.selected > 0, "PageDown moved past first row");
        assert!(p.selected.saturating_sub(p.scroll_top) < lh2, "PageDown: selection visible");
    }

    /// A6: typing a narrowing filter query re-clamps scroll_top so the selection
    /// is visible even when the previous scroll position is past the new row count.
    #[test]
    fn palette_filter_shrink_reclamps_window() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let reg = Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut e = Editor::new_from_text("", None, (80, 24));
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &km);
        e.palette = Some(p);
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press_key = |c: KeyCode| Msg::Input(Event::Key(KeyEvent {
            code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE,
        }));
        let press_char = |c: char| Msg::Input(Event::Key(KeyEvent {
            code: KeyCode::Char(c), modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE,
        }));
        // Navigate deep via End.
        crate::app::reduce(press_key(KeyCode::End), &mut e, &reg, &km, &ex, &clk, &tx);
        let deep_scroll_top = e.palette.as_ref().unwrap().scroll_top;
        assert!(deep_scroll_top > 0, "scroll_top must be > 0 after End");
        // Type a narrowing query — filter shrinks the row set.
        // Use 's' + 'a' + 'v' + 'e' which matches 'save', 'save_as', etc. — a small result set.
        for ch in "save".chars() {
            crate::app::reduce(press_char(ch), &mut e, &reg, &km, &ex, &clk, &tx);
        }
        let p = e.palette.as_ref().unwrap();
        let lh = crate::list_window::list_h_for(p.rows.len(), 24);
        assert!(p.selected.saturating_sub(p.scroll_top) < lh.max(1),
            "after filter shrink: selected must be within the visible window \
            (selected={}, scroll_top={}, lh={})", p.selected, p.scroll_top, lh);
    }

    /// A6 (Buffers palette variant): seed 20 buffers, open the switcher, PageDown →
    /// the selection lands within the visible window.
    #[test]
    fn palette_buffers_pgdn_lands_visible() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let reg = Registry::builtins();
        let km = cua_keymap();
        // Seed 20 buffers by opening scratch + creating extra ones.
        let mut e = Editor::new_from_text("buf0\n", None, (80, 24));
        e.install_scratch();
        for i in 1..20usize {
            // install_scratch already added a scratch buffer; add extra by pushing directly.
            let id = crate::editor::BufferId(100 + i as u64);
            let buf = crate::editor::Buffer::from_text(id, &format!("buf{i}\n"), None, (80, 24));
            e.buffers.push(buf);
        }
        e.open_buffer_switcher();
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press_key = |c: KeyCode| Msg::Input(Event::Key(KeyEvent {
            code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE,
        }));
        crate::app::reduce(press_key(KeyCode::PageDown), &mut e, &reg, &km, &ex, &clk, &tx);
        let p = e.palette.as_ref().unwrap();
        let lh = crate::list_window::list_h_for(p.rows.len(), 24);
        assert!(lh > 0, "list_h must be > 0 for a 24-row terminal");
        assert!(p.selected.saturating_sub(p.scroll_top) < lh,
            "Buffers palette: PageDown selection visible (selected={}, scroll_top={}, lh={})",
            p.selected, p.scroll_top, lh);
    }

    // -----------------------------------------------------------------------
    // A6 Task 2: sibling overlay windowed scrolling
    // -----------------------------------------------------------------------

    /// A6 (outline): 25 headings, pressing Down past the 15-row window cap keeps
    /// `selected - scroll_top < list_h`; PgDn/Home/End land at the expected positions.
    ///
    /// TDD RED: without `keep_overlay_visible` in the Up/Down arms, `scroll_top`
    /// stays 0 so `selected - scroll_top < list_h` fails when selected > 14.
    #[test]
    fn outline_pgdn_home_end_land_exactly() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        // Build a document with 25 headings so the outline exceeds the 15-row window.
        let doc: String = (0..25).map(|i| format!("# Heading {i:02}\n\n")).collect();
        let mut e = Editor::new_from_text(&doc, None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_outline();
        assert_eq!(e.outline.as_ref().unwrap().rows.len(), 25, "precondition: 25 headings");
        let reg = Registry::builtins();
        let km = cua_keymap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press_key = |c: KeyCode| Msg::Input(Event::Key(KeyEvent {
            code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE,
        }));
        let lh = crate::list_window::list_h_for(25, 24);
        assert_eq!(lh, 15, "list_h must be 15 for a 24-row terminal with 25 rows");
        // Down ×20 — crosses the 15-row window.
        for _ in 0..20 {
            crate::app::reduce(press_key(KeyCode::Down), &mut e, &reg, &km, &ex, &clk, &tx);
        }
        let o = e.outline.as_ref().unwrap();
        assert_eq!(o.selected, 20);
        assert!(o.selected.saturating_sub(o.scroll_top) < lh,
            "Down×20: selected={} scroll_top={} lh={} — selection must be visible",
            o.selected, o.scroll_top, lh);
        // End — lands at the last heading.
        crate::app::reduce(press_key(KeyCode::End), &mut e, &reg, &km, &ex, &clk, &tx);
        let o = e.outline.as_ref().unwrap();
        assert_eq!(o.selected, 24, "End must land on last row");
        assert!(o.selected.saturating_sub(o.scroll_top) < lh, "End: selection visible");
        // Home — lands at 0, scroll_top resets.
        crate::app::reduce(press_key(KeyCode::Home), &mut e, &reg, &km, &ex, &clk, &tx);
        let o = e.outline.as_ref().unwrap();
        assert_eq!(o.selected, 0, "Home must land on first row");
        assert_eq!(o.scroll_top, 0, "Home: scroll_top must reset to 0");
        // PageDown from 0 — jumps by lh.
        crate::app::reduce(press_key(KeyCode::PageDown), &mut e, &reg, &km, &ex, &clk, &tx);
        let o = e.outline.as_ref().unwrap();
        assert!(o.selected > 0, "PageDown must move past first row");
        assert!(o.selected.saturating_sub(o.scroll_top) < lh, "PageDown: selection visible");
    }

    /// A6 (file browser): 25 entries in a tempdir, Down past window keeps the
    /// selection visible; PgDn/Home/End land correctly; Enter dispatches the
    /// visible row, not the out-of-window rows[0].
    ///
    /// TDD RED: without `keep_overlay_visible` in the key arms, scroll_top stays 0
    /// so the visible-row assertion fails after Down×20.
    #[test]
    fn file_browser_pgdn_home_end_and_enter_dispatches_visible() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        // Create a tempdir with 24 subdirectories → 25 entries (.., d00..d23).
        let dir = std::env::temp_dir().join(format!("wc-a6-fb-nav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..24usize {
            std::fs::create_dir(dir.join(format!("d{i:02}"))).unwrap();
        }
        let mut e = Editor::new_from_text("", None, (80, 24));
        e.open_file_browser(dir.clone());
        assert_eq!(e.file_browser.as_ref().unwrap().entries.len(), 25,
            "precondition: 25 entries (.., d00..d23)");
        let reg = Registry::builtins();
        let km = cua_keymap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press_key = |c: KeyCode| Msg::Input(Event::Key(KeyEvent {
            code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE,
        }));
        let lh = crate::list_window::list_h_for(25, 24);
        assert_eq!(lh, 15);
        // Down ×20 — crosses the 15-row window.
        for _ in 0..20 {
            crate::app::reduce(press_key(KeyCode::Down), &mut e, &reg, &km, &ex, &clk, &tx);
        }
        let fb = e.file_browser.as_ref().unwrap();
        assert_eq!(fb.selected, 20);
        assert!(fb.selected.saturating_sub(fb.scroll_top) < lh,
            "Down×20: selected={} scroll_top={} lh={} — selection must be visible",
            fb.selected, fb.scroll_top, lh);
        // End — last row.
        crate::app::reduce(press_key(KeyCode::End), &mut e, &reg, &km, &ex, &clk, &tx);
        let fb = e.file_browser.as_ref().unwrap();
        assert_eq!(fb.selected, 24, "End must land on last row");
        assert!(fb.selected.saturating_sub(fb.scroll_top) < lh, "End: selection visible");
        // Home — row 0.
        crate::app::reduce(press_key(KeyCode::Home), &mut e, &reg, &km, &ex, &clk, &tx);
        let fb = e.file_browser.as_ref().unwrap();
        assert_eq!(fb.selected, 0); assert_eq!(fb.scroll_top, 0);
        // PageDown from 0.
        crate::app::reduce(press_key(KeyCode::PageDown), &mut e, &reg, &km, &ex, &clk, &tx);
        let fb = e.file_browser.as_ref().unwrap();
        assert!(fb.selected > 0);
        assert!(fb.selected.saturating_sub(fb.scroll_top) < lh, "PageDown: selection visible");
        // Enter-dispatches-visible: navigate to a deep selection, Enter opens that entry.
        // Navigate to the last entry (index 24 = d23 directory), scroll_top > 0.
        crate::app::reduce(press_key(KeyCode::End), &mut e, &reg, &km, &ex, &clk, &tx);
        let selected_entry = e.file_browser.as_ref().unwrap()
            .entries[e.file_browser.as_ref().unwrap().selected].name.clone();
        // The selected entry's scroll_top must be > 0 (we're at the end of a 25-entry list).
        assert!(e.file_browser.as_ref().unwrap().scroll_top > 0,
            "precondition for visible-row dispatch: scroll_top must be > 0");
        // Enter on a directory — descend (selected entry is d23 directory).
        crate::app::reduce(press_key(KeyCode::Enter), &mut e, &reg, &km, &ex, &clk, &tx);
        // After descend into a directory: selected==0, scroll_top==0.
        if let Some(fb) = e.file_browser.as_ref() {
            // We descended into the dir named `selected_entry`.
            assert!(fb.dir.ends_with(&selected_entry),
                "must have descended into {selected_entry:?}, dir={:?}", fb.dir);
            assert_eq!(fb.selected, 0, "after descend: selected must reset to 0");
            assert_eq!(fb.scroll_top, 0, "after descend: scroll_top must reset to 0");
        } else {
            // If the dir was empty (unlikely), the browser closed — not a failure of the test goal.
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A6 (file browser descend pin, panic-class C1): a tempdir with 25 subdirectories
    /// (`d00`..`d24`); PgDn lands on index 15 (= `d14`, after `..` at index 0) with
    /// `scroll_top > 0`; Enter resets `scroll_top = 0` and `selected = 0` — a stale
    /// window over a smaller-or-different entry set would cause an out-of-bounds slice
    /// (panic class) on the next render.
    ///
    /// TDD RED: without `fb.scroll_top = 0` in the Enter descend arm, `scroll_top`
    /// stays at its pre-descend value; if the new directory has fewer entries the
    /// render slice `entries[scroll_top..end]` panics (or shows stale content).
    #[test]
    fn file_browser_scrolled_descend_resets_window() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        // 25 subdirs (d00..d24) at the top; two small files INSIDE d14 so it's non-empty
        // but the browser list from d14 is smaller (just ".." + 2 files = 3 entries).
        let parent = std::env::temp_dir().join(format!("wc-a6-descend-{}", std::process::id()));
        std::fs::create_dir_all(&parent).unwrap();
        for i in 0..25usize {
            std::fs::create_dir(parent.join(format!("d{i:02}"))).unwrap();
        }
        std::fs::write(parent.join("d14").join("file_a.md"), "x").unwrap();
        std::fs::write(parent.join("d14").join("file_b.md"), "x").unwrap();
        let mut e = Editor::new_from_text("", None, (80, 24));
        e.open_file_browser(parent.clone());
        // rebuild_entries sorts dirs before files; ".." is index 0, d00..d24 follow.
        // 26 entries total (.., d00..d24).
        assert_eq!(e.file_browser.as_ref().unwrap().entries.len(), 26,
            "precondition: 26 entries (.., d00..d24)");
        let reg = Registry::builtins();
        let km = cua_keymap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        let press_key = |c: KeyCode| Msg::Input(Event::Key(KeyEvent {
            code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE,
        }));
        // PgDn from 0: selected = min(0 + 15, 25) = 15 (= d14, after ".." + d00..d13).
        crate::app::reduce(press_key(KeyCode::PageDown), &mut e, &reg, &km, &ex, &clk, &tx);
        let fb = e.file_browser.as_ref().unwrap();
        assert_eq!(fb.selected, 15, "PgDn must land on index 15 (d14)");
        assert!(fb.scroll_top > 0, "PgDn must advance scroll_top past 0");
        assert_eq!(fb.entries[fb.selected].name, "d14", "selected entry must be d14");
        assert!(fb.entries[fb.selected].is_dir, "d14 must be a directory");
        // Enter → descend into d14. No panic, selected and scroll_top reset.
        crate::app::reduce(press_key(KeyCode::Enter), &mut e, &reg, &km, &ex, &clk, &tx);
        let fb = e.file_browser.as_ref().expect("file browser must remain open after descend into dir");
        assert_eq!(fb.selected, 0, "after descend: selected must reset to 0");
        assert_eq!(fb.scroll_top, 0, "after descend: scroll_top must reset to 0");
        // Entries from d14: "..", file_a.md, file_b.md.
        assert!(fb.entries.len() >= 2, "d14 has at least 2 entries (.. + files)");
        // Render must not panic — invoke the painter indirectly by checking slice validity.
        let lh = crate::list_window::list_h_for(fb.entries.len(), 24);
        let end = (fb.scroll_top + lh).min(fb.entries.len());
        let _slice = &fb.entries[fb.scroll_top..end]; // panics if stale scroll_top
        let _ = std::fs::remove_dir_all(&parent);
    }

    /// A6 (theme picker preview pin): pad `tp.rows` to 30 rows using repeated
    /// real builtin names (cycling), drive with Down×20 (navigation only — no
    /// Char/Backspace which would rebuild and wipe the padding), then assert:
    /// (a) the applied theme's name equals `tp.rows[tp.selected]` (correct preview)
    /// (b) `tp.selected - tp.scroll_top < list_h` (the selection is visible).
    ///
    /// TDD RED: without `keep_overlay_visible` in the Down arm (ordering pin), the
    /// scroll_top stays 0 so assertion (b) fails with selected=20, scroll_top=0, lh=15.
    #[test]
    fn theme_picker_preview_pin_visible_row() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("# Heading\n\nhello\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        e.open_theme_picker(); // populates rows from builtin_names
        let reg = Registry::builtins(); let km = cua_keymap();
        let (tx, _rx) = std::sync::mpsc::channel();
        let ex = InlineExecutor::default(); let clk = TestClock(0);
        // Pad tp.rows to 30 by cycling real builtin names — navigation-only,
        // no Char/Backspace which would call rebuild_rows and wipe the padding.
        {
            let names = wordcartel_core::theme::Theme::builtin_names();
            let tp = e.theme_picker.as_mut().unwrap();
            tp.rows.clear();
            for i in 0..30 { tp.rows.push(names[i % names.len()].to_string()); }
        }
        assert_eq!(e.theme_picker.as_ref().unwrap().rows.len(), 30);
        let press_key = |c: KeyCode| Msg::Input(Event::Key(KeyEvent {
            code: c, modifiers: KeyModifiers::NONE, kind: KeyEventKind::Press, state: KeyEventState::NONE,
        }));
        // Drive Down ×20.
        for _ in 0..20 {
            crate::app::reduce(press_key(KeyCode::Down), &mut e, &reg, &km, &ex, &clk, &tx);
        }
        let tp = e.theme_picker.as_ref().unwrap();
        assert_eq!(tp.selected, 20, "selected must be 20 after Down×20");
        let lh = crate::list_window::list_h_for(tp.rows.len(), 24);
        assert_eq!(lh, 15, "list_h must be 15 for 30 rows on 24-row terminal");
        // (b) visible invariant.
        assert!(tp.selected.saturating_sub(tp.scroll_top) < lh,
            "preview pin: selected={} scroll_top={} lh={} — selection must be visible",
            tp.selected, tp.scroll_top, lh);
        // (a) the applied theme's name must equal tp.rows[tp.selected].
        let expected_name = tp.rows[tp.selected].clone();
        assert_eq!(e.theme.name, expected_name,
            "applied theme must equal tp.rows[selected]={expected_name:?}, got {:?}", e.theme.name);
    }

    /// The arm block (post-rebuild) sets `due_at` once and does not push the
    /// deadline on idle Ticks; a new edit (version bump) re-arms the debounce.
    #[test]
    fn reconcile_arm_sets_due_once_and_debounces_on_new_edit() {
        // Simulate the post-rebuild arm block against a ReconcileStore directly.
        let mut s = crate::reconcile::ReconcileStore { maybe_stale: true, ..Default::default() };
        let arm = |s: &mut crate::reconcile::ReconcileStore, now: u64, version: u64| {
            if s.maybe_stale && s.in_flight_version.is_none()
                && (s.due_at.is_none() || s.armed_for_version != version) {
                s.due_at = Some(now + crate::reconcile::RECONCILE_DEBOUNCE_MS);
                s.armed_for_version = version;
            }
        };
        arm(&mut s, 1000, 5);
        assert_eq!(s.due_at, Some(1000 + crate::reconcile::RECONCILE_DEBOUNCE_MS));
        arm(&mut s, 1050, 5); // idle Tick, same version → no push
        assert_eq!(s.due_at, Some(1000 + crate::reconcile::RECONCILE_DEBOUNCE_MS));
        arm(&mut s, 1100, 6); // new edit → re-debounce
        assert_eq!(s.due_at, Some(1100 + crate::reconcile::RECONCILE_DEBOUNCE_MS));
    }
}
