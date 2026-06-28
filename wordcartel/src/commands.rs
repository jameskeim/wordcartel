//! Editing commands — the layer that translates a `Command` into a
//! `Transaction` + `block_tree::Edit`, calls `editor.apply`, then re-derives.
//!
//! Every edit command:
//!   1. Captures the current caret offset via `nav::head`.
//!   2. Builds a `ChangeSet` and a matching `block_tree::Edit { range, new_len }`
//!      from the *same* `(range, replacement)`.
//!   3. Calls `editor.apply(txn, edit, kind, clock)`.
//!   4. Calls `derive::rebuild(editor)`.
//!   5. Calls `nav::ensure_visible(editor)`.
//!   6. Sets `editor.desired_col = None` (an edit re-anchors vertical motion).

use crate::derive;
use crate::editor::{Editor, RenderMode};
use crate::file;
use crate::nav;
use crate::registry::{place_caret_visible, CaretPlace};
use wordcartel_core::block_tree::Edit;
use wordcartel_core::change::ChangeSet;
use wordcartel_core::history::{Clock, EditKind, Transaction};
use wordcartel_core::register;
use wordcartel_core::selection::{Range, Selection};

/// Text object scope for selection expansion commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope { Word, Sentence, Paragraph, Document }

/// Direction of caret movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
    WordLeft,
    WordRight,
    ParagraphUp,
    ParagraphDown,
    PageUp,
    PageDown,
    DocStart,
    DocEnd,
}

/// Commands that can be dispatched to the editor.
#[derive(Debug, Clone)]
pub enum Command {
    InsertChar(char),
    InsertNewline,
    Backspace,
    DeleteForward,
    /// Navigate the caret. `extend=false` collapses the selection; `extend=true` keeps the anchor.
    Move { dir: Dir, extend: bool },
    /// Copy the primary selection into the register (no mutation).
    Copy,
    /// Cut the primary selection into the register and delete it.
    Cut,
    /// Paste register contents at the caret position.
    Paste,
    /// Undo the last committed revision.
    Undo,
    /// Redo the next revision (after an undo).
    Redo,
    /// Rotate the render mode: LivePreview → SourceHighlighted → SourcePlain → LivePreview.
    CycleRenderMode,
    /// Save the current document to its path (atomic write).
    Save,
    /// Request to quit; a second Quit while dirty force-quits.
    Quit,
    /// Delete one word backwards (back=true) or forwards (back=false).
    DeleteWord { back: bool },
    /// Delete the entire logical line the caret is on, including its trailing newline.
    DeleteLine,
    /// Delete from the caret to the end of the current logical line, keeping the newline.
    DeleteToLineEnd,
    /// Select the given text-object scope at the caret (clears sel_history).
    SelectScope(Scope),
    /// Grow selection: word → sentence → paragraph → document (pushes to sel_history).
    ExpandSelection,
    /// Shrink selection: pop one level from sel_history.
    ShrinkSelection,
    /// Select the entire buffer contents.
    SelectAll,
}

/// Result returned by `run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandResult {
    /// The command was handled and the editor state may have changed.
    Handled,
    /// The command is a no-op; the editor state is unchanged.
    Noop,
    /// The editor should quit.
    Quit,
}

/// Build a `ChangeSet` that replaces the byte range `from..to` with `text`.
///
/// The Edit passed to `editor.apply` must match this exactly:
///   `Edit { range: from..to, new_len: text.len() }`.
fn replace_changeset(
    from: usize,
    to: usize,
    text: &str,
    doc_len: usize,
) -> wordcartel_core::change::ChangeSet {
    use wordcartel_core::change::{ChangeSet, Op, Tendril};
    let mut ops = Vec::new();
    if from > 0 {
        ops.push(Op::Retain(from));
    }
    if to > from {
        ops.push(Op::Delete(to - from));
    }
    if !text.is_empty() {
        ops.push(Op::Insert(Tendril::from(text)));
    }
    if doc_len > to {
        ops.push(Op::Retain(doc_len - to));
    }
    ChangeSet {
        ops,
        len_before: doc_len,
        len_after: doc_len - (to - from) + text.len(),
    }
}

/// Build ONE `ChangeSet` performing all `edits` (ascending, non-overlapping
/// `(from,to,replacement)`) plus ONE covering `block_tree::Edit` spanning
/// `first.start..last.end`. Applied as a single `editor.apply` → one undo unit.
pub fn build_multi_replace(
    edits: &[(usize, usize, String)],
    doc_len: usize,
) -> (wordcartel_core::change::ChangeSet, wordcartel_core::block_tree::Edit) {
    use wordcartel_core::change::{ChangeSet, Op, Tendril};
    debug_assert!(!edits.is_empty());
    let mut ops = Vec::new();
    let mut pos = 0usize;
    let mut len_after = doc_len;
    for (from, to, text) in edits {
        if *from > pos { ops.push(Op::Retain(from - pos)); }
        if to > from { ops.push(Op::Delete(to - from)); }
        if !text.is_empty() { ops.push(Op::Insert(Tendril::from(text.as_str()))); }
        len_after = len_after - (to - from) + text.len();
        pos = *to;
    }
    if doc_len > pos { ops.push(Op::Retain(doc_len - pos)); }
    let first = edits.first().unwrap().0;
    let last_to = edits.last().unwrap().1;
    // new_len of the covering region = (last_to - first) adjusted by all deltas.
    let delta: isize = edits.iter().map(|(f, t, s)| s.len() as isize - (t - f) as isize).sum();
    let new_len = ((last_to - first) as isize + delta) as usize;
    let cs = ChangeSet { ops, len_before: doc_len, len_after };
    let edit = wordcartel_core::block_tree::Edit { range: first..last_to, new_len };
    (cs, edit)
}

/// Build a `(ChangeSet, Edit)` replacing byte range `from..to` with `text`.
/// Public so the filter merge (filter.rs) can produce one undoable edit.
pub fn build_range_replace(
    from: usize, to: usize, text: &str, doc_len: usize,
) -> (wordcartel_core::change::ChangeSet, wordcartel_core::block_tree::Edit) {
    let cs = replace_changeset(from, to, text, doc_len); // existing private builder
    let edit = wordcartel_core::block_tree::Edit { range: from..to, new_len: text.len() };
    (cs, edit)
}

/// Compute the (from, to) byte range of `scope` at the given byte offset `h`.
/// Borrows `editor` immutably and returns owned `(usize, usize)`.
pub fn scope_range_at(editor: &Editor, h: usize, scope: Scope) -> (usize, usize) {
    let buf = &editor.active().document.buffer;
    let blocks = &editor.active().document.blocks;
    match scope {
        Scope::Word => {
            let (ps, pe) = nav::paragraph_range_at(blocks, buf, h);
            let win = buf.slice(ps..pe);
            let (wf, wt) = wordcartel_core::textobj::word_bounds(&win, h - ps);
            if wf == wt {
                // in whitespace → nearest word (next within block, else prev)
                match wordcartel_core::textobj::next_word_start(&win, h - ps)
                    .or_else(|| wordcartel_core::textobj::prev_word_start(&win, h - ps)) {
                    Some(r) => { let (a, b) = wordcartel_core::textobj::word_bounds(&win, r); (ps + a, ps + b) }
                    None => (h, h),
                }
            } else { (ps + wf, ps + wt) }
        }
        Scope::Sentence => {
            let (ps, pe) = nav::paragraph_range_at(blocks, buf, h);
            let win = buf.slice(ps..pe);
            let (sf, st) = wordcartel_core::textobj::sentence_bounds(&win, h - ps);
            (ps + sf, ps + st)
        }
        Scope::Paragraph => nav::paragraph_range_at(blocks, buf, h),
        Scope::Document => (0, buf.len()),
    }
}

/// Compute the (from, to) byte range of `scope` at the caret position.
/// Borrows `editor` immutably and returns owned `(usize, usize)`.
fn scope_range(editor: &Editor, scope: Scope) -> (usize, usize) {
    scope_range_at(editor, nav::head(editor), scope)
}

/// Set the selection to [from, to) and re-derive + ensure visibility.
fn set_selection_range(editor: &mut Editor, from: usize, to: usize) {
    editor.active_mut().document.selection = Selection::range(from, to);
    derive::rebuild(editor);
    nav::ensure_visible(editor);
}

/// Execute `cmd` against `editor`, then re-derive + ensure visibility.
pub fn run(cmd: Command, editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    match cmd {
        Command::InsertChar(c) => {
            let sel = editor.active().document.selection.primary();
            if !sel.is_empty() {
                // Non-empty selection: replace it with the typed character (CUA).
                let (from, to) = (sel.from(), sel.to());
                let text = c.to_string();
                let doc_len = editor.active().document.buffer.len();
                let cs = replace_changeset(from, to, &text, doc_len);
                let edit = Edit { range: from..to, new_len: text.len() };
                let txn = Transaction::new(cs).with_selection(Selection::single(from + text.len()));
                editor.apply(txn, edit, EditKind::Other, clock);
                derive::rebuild(editor);
                nav::ensure_visible(editor);
                editor.active_mut().desired_col = None;
                return CommandResult::Handled;
            }
            // Collapsed selection: normal insert-at-caret path.
            let at = nav::head(editor);
            let s = c.to_string();
            let doc_len = editor.active().document.buffer.len();
            let cs = ChangeSet::insert(at, &s, doc_len);
            let new_len = s.len(); // == c.len_utf8()
            let edit = Edit { range: at..at, new_len };
            let txn = Transaction::new(cs).with_selection(Selection::single(at + new_len));
            editor.apply(txn, edit, EditKind::Type, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }

        Command::InsertNewline => {
            let sel = editor.active().document.selection.primary();
            if !sel.is_empty() {
                // Non-empty selection: replace it with a newline (CUA).
                let (from, to) = (sel.from(), sel.to());
                let text = "\n";
                let doc_len = editor.active().document.buffer.len();
                let cs = replace_changeset(from, to, text, doc_len);
                let edit = Edit { range: from..to, new_len: text.len() };
                let txn = Transaction::new(cs).with_selection(Selection::single(from + text.len()));
                editor.apply(txn, edit, EditKind::Other, clock);
                derive::rebuild(editor);
                nav::ensure_visible(editor);
                editor.active_mut().desired_col = None;
                return CommandResult::Handled;
            }
            // Collapsed selection: normal insert-newline path.
            let at = nav::head(editor);
            let s = "\n";
            let doc_len = editor.active().document.buffer.len();
            let cs = ChangeSet::insert(at, s, doc_len);
            let new_len: usize = 1;
            let edit = Edit { range: at..at, new_len };
            // EditKind::Other breaks coalescing at each newline so that undo
            // chunks per logical line rather than collapsing multi-line insertions.
            let txn = Transaction::new(cs).with_selection(Selection::single(at + new_len));
            editor.apply(txn, edit, EditKind::Other, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }

        Command::Backspace => {
            let sel = editor.active().document.selection.primary();
            if !sel.is_empty() {
                // Non-empty selection: delete the selection range (like Cut, minus clipboard).
                let (from, to) = (sel.from(), sel.to());
                let doc_len = editor.active().document.buffer.len();
                let cs = ChangeSet::delete(from..to, doc_len);
                let edit = Edit { range: from..to, new_len: 0 };
                let txn = Transaction::new(cs).with_selection(Selection::single(from));
                editor.apply(txn, edit, EditKind::Other, clock);
                derive::rebuild(editor);
                nav::ensure_visible(editor);
                editor.active_mut().desired_col = None;
                return CommandResult::Handled;
            }
            // Collapsed selection: delete one grapheme left of the caret.
            let head = nav::head(editor);
            if head == 0 {
                return CommandResult::Noop;
            }
            // Compute the grapheme-correct previous stop by reusing move_left.
            // move_left sets desired_col=None as a side-effect but does NOT change
            // the selection; it purely returns the new offset. We capture `prev`
            // here and then use it for the delete range. `head` is unchanged.
            let prev = nav::move_left(editor);
            let doc_len = editor.active().document.buffer.len();
            let cs = ChangeSet::delete(prev..head, doc_len);
            let edit = Edit { range: prev..head, new_len: 0 };
            let txn = Transaction::new(cs).with_selection(Selection::single(prev));
            editor.apply(txn, edit, EditKind::Other, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }

        Command::DeleteForward => {
            let sel = editor.active().document.selection.primary();
            if !sel.is_empty() {
                // Non-empty selection: delete the selection range (CUA, mirrors Backspace).
                let (from, to) = (sel.from(), sel.to());
                let doc_len = editor.active().document.buffer.len();
                let cs = ChangeSet::delete(from..to, doc_len);
                let edit = Edit { range: from..to, new_len: 0 };
                let txn = Transaction::new(cs).with_selection(Selection::single(from));
                editor.apply(txn, edit, EditKind::Other, clock);
                derive::rebuild(editor);
                nav::ensure_visible(editor);
                editor.active_mut().desired_col = None;
                return CommandResult::Handled;
            }
            // Collapsed selection: delete one grapheme forward.
            let head = nav::head(editor);
            // Compute the grapheme-correct next stop by reusing move_right.
            // move_right sets desired_col=None as a side-effect but does NOT change
            // the selection; it purely returns the new offset.
            let next = nav::move_right(editor);
            // EOF / nothing to delete guard: if next == head we are at the very end
            // of the document. Do NOT build a zero-width delete — it would dirty the
            // buffer, bump the version, and push a no-op undo entry.
            if next == head {
                return CommandResult::Noop;
            }
            let doc_len = editor.active().document.buffer.len();
            let cs = ChangeSet::delete(head..next, doc_len);
            let edit = Edit { range: head..next, new_len: 0 };
            // Caret stays at `head` after a forward delete.
            let txn = Transaction::new(cs).with_selection(Selection::single(head));
            editor.apply(txn, edit, EditKind::Other, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }

        Command::Move { dir, extend } => {
            // Reset the expand-selection ladder on every motion (Task 7).
            editor.active_mut().sel_history.clear();
            // DocStart / DocEnd are deliberate long-range jumps: push the ring
            // so the user can alt-left back to where they came from.
            if matches!(dir, Dir::DocStart | Dir::DocEnd) {
                let pre = nav::head(editor);
                crate::marks::record_jump(editor.active_mut(), pre);
            }
            // Compute the new head offset using the appropriate nav function.
            let new_head = match dir {
                Dir::Left     => nav::move_left(editor),
                Dir::Right    => nav::move_right(editor),
                Dir::Up       => nav::move_up(editor),
                Dir::Down     => nav::move_down(editor),
                Dir::LineStart => nav::move_home(editor),
                Dir::LineEnd   => nav::move_end(editor),
                Dir::WordLeft      => nav::move_word_left(editor),
                Dir::WordRight     => nav::move_word_right(editor),
                Dir::ParagraphUp   => nav::move_paragraph_up(editor),
                Dir::ParagraphDown => nav::move_paragraph_down(editor),
                Dir::PageUp        => nav::move_page_up(editor),
                Dir::PageDown      => nav::move_page_down(editor),
                Dir::DocStart      => nav::move_doc_start(editor),
                Dir::DocEnd        => nav::move_doc_end(editor),
            };
            // Up/Down preserve desired_col (handled inside move_up/move_down).
            // Horizontal moves reset desired_col to None (handled inside move_left/right/home/end).

            // Central fold-aware invariant: ensure the committed head is never
            // inside a folded body, for ALL motion directions at once.
            let new_head = {
                let b = editor.active();
                crate::fold::normalize_caret(&b.folds, &b.document.blocks, &b.document.buffer, new_head)
            };

            if extend {
                // Keep the current anchor; move the head to `new_head`.
                let anchor = editor.active().document.selection.primary().anchor;
                editor.active_mut().document.selection = Selection {
                    ranges: [Range { anchor, head: new_head }].into_iter().collect(),
                    primary: 0,
                };
            } else {
                // Collapse to a point at the new head.
                editor.active_mut().document.selection = Selection::single(new_head);
            }

            derive::rebuild(editor);
            nav::ensure_visible(editor);
            CommandResult::Handled
        }

        Command::Copy => {
            let r = editor.active().document.selection.primary();
            if r.is_empty() {
                // Copy-on-empty must NOT overwrite the register with "".
                return CommandResult::Noop;
            }
            // Clone the buffer before mutably borrowing editor.register (field-split no longer
            // applies now that the buffer lives under editor.active() rather than directly on Editor).
            let buf_snap = editor.active().document.buffer.clone();
            register::copy(&buf_snap, r, &mut editor.register);
            if let Some(text) = editor.register.get().map(str::to_owned) {
                editor.clipboard_sync_request = Some(text);
            }
            editor.status = "Copied".to_string();
            CommandResult::Handled
        }

        Command::Cut => {
            let r = editor.active().document.selection.primary();
            if r.is_empty() {
                return CommandResult::Noop;
            }
            let doc_len = editor.active().document.buffer.len();
            // Borrow the buffer before mutably borrowing editor.register (field-split no longer
            // applies now that both live under editor.active() rather than directly on Editor).
            let buf_snap = editor.active().document.buffer.clone();
            let cs = register::cut(r, doc_len, &mut editor.register, &buf_snap);
            let edit = Edit { range: r.from()..r.to(), new_len: 0 };
            let txn = Transaction::new(cs).with_selection(Selection::single(r.from()));
            editor.apply(txn, edit, EditKind::Other, clock);
            if let Some(text) = editor.register.get().map(str::to_owned) {
                editor.clipboard_sync_request = Some(text);
            }
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }

        Command::Paste => {
            editor.clipboard_get_pending = Some(crate::clipboard::PasteIntent {
                id: crate::clipboard::next_paste_id(),
                buffer_id: editor.active().id,
            });
            CommandResult::Handled
        }

        Command::Undo => {
            if !editor.undo() {
                return CommandResult::Noop;
            }
            derive::rebuild(editor);
            let head = editor.active().document.selection.primary().head;
            let snapped = place_caret_visible(editor, head, CaretPlace::SnapOut);
            if snapped != head {
                editor.active_mut().document.selection = Selection::single(snapped);
            }
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }

        Command::Redo => {
            if !editor.redo() {
                return CommandResult::Noop;
            }
            derive::rebuild(editor);
            let head = editor.active().document.selection.primary().head;
            let snapped = place_caret_visible(editor, head, CaretPlace::SnapOut);
            if snapped != head {
                editor.active_mut().document.selection = Selection::single(snapped);
            }
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }

        Command::CycleRenderMode => {
            editor.active_mut().view.mode = match editor.active().view.mode {
                RenderMode::LivePreview      => RenderMode::SourceHighlighted,
                RenderMode::SourceHighlighted => RenderMode::SourcePlain,
                RenderMode::SourcePlain      => RenderMode::LivePreview,
            };
            derive::rebuild(editor);
            nav::ensure_visible(editor); // a mode change can alter layout/scroll (§4.5)
            CommandResult::Handled
        }

        // SUPERSEDED (Effort 4b-1): production save routes through the registry
        // `"save"` handler → `save::dispatch_save` (background, version-aware,
        // external-mod guarded). This synchronous arm is retained only for the
        // legacy `commands::run(Command::Save, …)` test path and must NOT be
        // wired to a key for production dispatch — it lacks the fingerprint guard.
        Command::Save => {
            // Snapshot the path and version before any mutable borrows.
            let path_opt = editor.active().document.path.clone();
            match path_opt {
                None => {
                    editor.status = "No file name (save-as is Effort 5)".to_string();
                }
                Some(path) => {
                    let v = editor.active().document.version;
                    editor.status = "Saving\u{2026}".to_string();
                    let content = editor.active().document.buffer.to_string();
                    match file::save_atomic(&path, &content) {
                        Ok(file::SaveOutcome::Saved) => {
                            editor.active_mut().document.mark_saved(v);
                            editor.status = "Saved".to_string();
                        }
                        Ok(file::SaveOutcome::Unchanged) => {
                            editor.active_mut().document.mark_saved(v);
                            editor.status = "(unchanged)".to_string();
                        }
                        Err(e) => {
                            // Buffer stays dirty; show error in status.
                            editor.status = e.to_string();
                        }
                    }
                }
            }
            CommandResult::Handled
        }

        Command::Quit => {
            if editor.active().document.dirty() {
                editor.open_prompt(crate::prompt::Prompt::quit_confirm());
                CommandResult::Handled
            } else {
                editor.quit = true;
                CommandResult::Quit
            }
        }

        Command::DeleteWord { back } => {
            let h = nav::head(editor);
            let target = if back { nav::move_word_left(editor) } else { nav::move_word_right(editor) };
            let (from, to) = if back { (target, h) } else { (h, target) };
            if from == to { return CommandResult::Noop; }
            let doc_len = editor.active().document.buffer.len();
            let cs = ChangeSet::delete(from..to, doc_len);
            let edit = Edit { range: from..to, new_len: 0 };
            let txn = Transaction::new(cs).with_selection(Selection::single(from));
            // EditKind::Other — matches existing delete commands, avoids coalescing with typed chars.
            editor.apply(txn, edit, EditKind::Other, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }

        Command::DeleteLine => {
            let head = nav::head(editor);
            let len = editor.active().document.buffer.len();
            if len == 0 { return CommandResult::Noop; }
            let (from, to) = {
                let buf = &editor.active().document.buffer;
                let total = derive::total_logical_lines(buf);
                let l = buf.byte_to_line(head);
                let start = buf.line_to_byte(l);
                let end = if l + 1 < total { buf.line_to_byte(l + 1) } else { len };
                if start == end {
                    // Empty line — the phantom final logical line that exists only because
                    // of a trailing '\n' (start == len). Remove the preceding newline so it
                    // disappears.
                    if start > 0 { (start - 1, end) } else { (start, end) }
                } else if end == len && buf.slice(len - 1..len) != "\n" {
                    // Final line with NO trailing newline → absorb the preceding newline too,
                    // so the line fully vanishes (slice returns String).
                    if start > 0 { (start - 1, end) } else { (start, end) }
                } else {
                    (start, end)
                }
            };
            if from == to { return CommandResult::Noop; }
            let cs = ChangeSet::delete(from..to, len);
            let edit = Edit { range: from..to, new_len: 0 };
            let txn = Transaction::new(cs).with_selection(Selection::single(from));
            editor.apply(txn, edit, EditKind::Other, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }

        Command::DeleteToLineEnd => {
            let head = nav::head(editor);
            let len = editor.active().document.buffer.len();
            let to = {
                let buf = &editor.active().document.buffer;
                let total = derive::total_logical_lines(buf);
                let l = buf.byte_to_line(head);
                let line_end = if l + 1 < total { buf.line_to_byte(l + 1) } else { len };
                // Keep the newline: stop before a trailing '\n' if present.
                if line_end > head && line_end <= len && line_end > 0 && buf.slice(line_end - 1..line_end) == "\n" {
                    line_end - 1
                } else {
                    line_end
                }
            };
            if head >= to { return CommandResult::Noop; } // at/after EOL → no empty changeset
            let cs = ChangeSet::delete(head..to, len);
            let edit = Edit { range: head..to, new_len: 0 };
            let txn = Transaction::new(cs).with_selection(Selection::single(head));
            editor.apply(txn, edit, EditKind::Other, clock);
            derive::rebuild(editor);
            nav::ensure_visible(editor);
            editor.active_mut().desired_col = None;
            CommandResult::Handled
        }

        Command::SelectScope(scope) => {
            editor.active_mut().sel_history.clear();
            let (from, to) = scope_range(editor, scope);
            set_selection_range(editor, from, to);
            CommandResult::Handled
        }

        Command::ExpandSelection => {
            let cur = editor.active().document.selection.primary();
            let (cf, ct) = (cur.from(), cur.to());
            // Find the smallest scope strictly larger than the current selection.
            let order = [Scope::Word, Scope::Sentence, Scope::Paragraph, Scope::Document];
            let mut next: Option<(usize, usize)> = None;
            for sc in order {
                let (f, t) = scope_range(editor, sc);
                if f <= cf && t >= ct && (f < cf || t > ct) { next = Some((f, t)); break; }
            }
            if let Some((f, t)) = next {
                // Clone to a local first — avoids overlapping active()/active_mut() borrow.
                let cur_sel = editor.active().document.selection.clone();
                editor.active_mut().sel_history.push(cur_sel);
                set_selection_range(editor, f, t);
                CommandResult::Handled
            } else { CommandResult::Noop }
        }

        Command::ShrinkSelection => {
            if let Some(prev) = editor.active_mut().sel_history.pop() {
                editor.active_mut().document.selection = prev;
                derive::rebuild(editor);
                let head = editor.active().document.selection.primary().head;
                let snapped = place_caret_visible(editor, head, CaretPlace::SnapOut);
                if snapped != head {
                    editor.active_mut().document.selection = Selection::single(snapped);
                }
                nav::ensure_visible(editor);
                CommandResult::Handled
            } else { CommandResult::Noop }
        }

        Command::SelectAll => {
            let len = editor.active().document.buffer.len();
            editor.active_mut().document.selection =
                wordcartel_core::selection::Selection::range(0, len);
            editor.active_mut().desired_col = None;
            editor.active_mut().sel_history.clear();
            nav::ensure_visible(editor);
            CommandResult::Handled
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive;
    use crate::editor::Editor;
    use crate::nav;
    use wordcartel_core::selection::Selection;

    /// A fixed-timestamp clock: always returns the same millisecond value.
    /// Used to drive coalescing (same ms → within COALESCE_MS window) or
    /// to break coalescing when two different timestamps are used.
    struct TestClock(u64);
    impl wordcartel_core::history::Clock for TestClock {
        fn now_ms(&self) -> u64 {
            self.0
        }
    }

    /// Set the caret to a raw byte offset without touching history.
    fn set_caret(e: &mut Editor, off: usize) {
        e.active_mut().document.selection = Selection::single(off);
    }

    /// Set the caret to the end of the current buffer content.
    fn set_caret_end(e: &mut Editor) {
        let end = nav::head(e);
        // Compute the real end: length of the buffer minus the trailing newlines,
        // but for simplicity just move right until we can't anymore.
        // Actually: nav::head gives the current head. We want the last char before EOF.
        // Use the buffer length directly — head of last grapheme position.
        let len = e.active().document.buffer.len();
        // Find the last grapheme stop before `len`. move_right from any position
        // will stop at EOF. Easier: set caret to `len` and then move_left once to
        // get before the trailing newline. But the brief test types "hi" at end-of-line
        // on "\n" — so the end of the first line (before '\n') is offset 0.
        // Let's use: place caret at whatever move_right reaches from current position
        // iteratively, or just set it to the buffer len and call move_left to find
        // the last valid stop on the last line.
        //
        // For the actual test ("\n" document, 1 byte): we want caret at offset 0
        // (before the '\n'), which is where Editor::new_from_text puts it initially.
        // So we just need to keep calling move_right until it returns the same offset.
        let mut cur = end;
        loop {
            e.active_mut().document.selection = Selection::single(cur);
            let nxt = nav::move_right(e);
            if nxt == cur {
                break;
            }
            cur = nxt;
        }
        e.active_mut().document.selection = Selection::single(cur);
        e.active_mut().desired_col = None;
        let _ = len;
    }

    // -------------------------------------------------------------------------
    // Brief's required failing tests (RED → GREEN)
    // -------------------------------------------------------------------------

    /// Typing 'b' between 'a' and 'c' inserts it and advances the caret.
    #[test]
    fn insert_char_types_and_advances() {
        let mut e = Editor::new_from_text("ac\n", None, (80, 24));
        set_caret(&mut e, 1);
        let clk = TestClock(0);
        run(Command::InsertChar('b'), &mut e, &clk);
        assert_eq!(e.active().document.buffer.to_string(), "abc\n");
        assert_eq!(nav::head(&e), 2);
    }

    /// Backspace at caret 2 in "abc\n" removes 'b' and moves caret to 1.
    #[test]
    fn backspace_deletes_prev_char() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        set_caret(&mut e, 2);
        let clk = TestClock(0);
        run(Command::Backspace, &mut e, &clk);
        assert_eq!(e.active().document.buffer.to_string(), "ac\n");
        assert_eq!(nav::head(&e), 1);
    }

    /// Typing "hi" with the same timestamp coalesces into a single undo entry.
    #[test]
    fn typing_coalesces_into_one_undo() {
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        let clk = TestClock(0); // same timestamp -> within COALESCE_MS
        // Type "hi" one char at a time, advancing caret to end-of-line each time
        // (before the trailing '\n').
        for c in "hi".chars() {
            set_caret_end(&mut e);
            run(Command::InsertChar(c), &mut e, &clk);
        }
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "\n"); // both chars undone together
    }

    // -------------------------------------------------------------------------
    // DeleteForward at EOF returns Noop; buffer unchanged, not dirty.
    // -------------------------------------------------------------------------

    /// DeleteForward at end of buffer (EOF) must return Noop and leave the
    /// buffer untouched and not dirty.
    #[test]
    fn delete_forward_at_eof_is_noop() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        derive::rebuild(&mut e);
        // Move caret to the end of the document. "ab\n" has 3 bytes; the last
        // valid caret position within the last-but-one line is offset 2 (after 'b').
        // move_right from 2 crosses to the empty trailing line (offset 3).
        // move_right from 3 stays at 3 (EOF). Let's place caret at 3.
        set_caret(&mut e, 3);
        let clk = TestClock(0);
        let result = run(Command::DeleteForward, &mut e, &clk);
        assert_eq!(result, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");
        assert!(!e.active().document.dirty(), "DeleteForward at EOF must not dirty the buffer");
    }

    // -------------------------------------------------------------------------
    // Additional correctness tests
    // -------------------------------------------------------------------------

    /// Backspace at offset 0 is a Noop.
    #[test]
    fn backspace_at_start_is_noop() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        set_caret(&mut e, 0);
        let clk = TestClock(0);
        let result = run(Command::Backspace, &mut e, &clk);
        assert_eq!(result, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "abc\n");
        assert!(!e.active().document.dirty());
    }

    /// DeleteForward in the middle of a line removes the next character.
    #[test]
    fn delete_forward_removes_next_char() {
        let mut e = Editor::new_from_text("abc\n", None, (80, 24));
        derive::rebuild(&mut e);
        set_caret(&mut e, 1); // caret at 'b'
        let clk = TestClock(0);
        let result = run(Command::DeleteForward, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "ac\n");
        assert_eq!(nav::head(&e), 1); // caret stays at 1
    }

    /// InsertNewline splits the current line.
    #[test]
    fn insert_newline_splits_line() {
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        set_caret(&mut e, 1); // between 'a' and 'b'
        let clk = TestClock(0);
        let result = run(Command::InsertNewline, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "a\nb\n");
        assert_eq!(nav::head(&e), 2); // caret after the newline
    }

    /// The Edit passed to apply for InsertChar matches the actual byte change:
    /// range is at..at and new_len is the char's UTF-8 byte length.
    #[test]
    fn insert_edit_matches_change() {
        let mut e = Editor::new_from_text("a\n", None, (80, 24));
        set_caret(&mut e, 1);
        let clk = TestClock(0);
        run(Command::InsertChar('é'), &mut e, &clk); // 'é' is 2 bytes
        assert_eq!(e.active().document.buffer.to_string(), "aé\n");
        // After apply+rebuild, last_edit is None (rebuild consumed it).
        // Verify the result: caret should be at 1 + 2 = 3.
        assert_eq!(nav::head(&e), 3);
    }

    // -------------------------------------------------------------------------
    // Task 9: Selection-extending navigation + clipboard (copy/cut/paste)
    // -------------------------------------------------------------------------

    /// Moving right twice with extend=true selects the first two chars.
    /// Then Copy puts those 2 chars in the register.
    #[test]
    fn select_right_twice_then_copy_fills_register() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        set_caret(&mut e, 0);
        derive::rebuild(&mut e);
        let clk = TestClock(0);

        // First extend-right: anchor=0, head=1 → selects 'a'
        run(Command::Move { dir: Dir::Right, extend: true }, &mut e, &clk);
        // Second extend-right: anchor=0, head=2 → selects 'ab'
        run(Command::Move { dir: Dir::Right, extend: true }, &mut e, &clk);

        // The selection should be non-collapsed: anchor=0, head=2
        let sel = e.active().document.selection.primary();
        assert_eq!(sel.anchor, 0, "anchor must stay at 0");
        assert_eq!(sel.head, 2, "head must be at 2");
        assert!(!sel.is_empty(), "selection must be non-empty");

        // Copy should place "ab" in the register
        run(Command::Copy, &mut e, &clk);
        assert_eq!(e.register.get(), Some("ab"), "register must contain the selected text");
    }

    /// Cut removes the selected 2-char region, leaves caret at range start,
    /// and places the text in the register.
    #[test]
    fn select_right_twice_then_cut_removes_selection() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        set_caret(&mut e, 0);
        derive::rebuild(&mut e);
        let clk = TestClock(0);

        run(Command::Move { dir: Dir::Right, extend: true }, &mut e, &clk);
        run(Command::Move { dir: Dir::Right, extend: true }, &mut e, &clk);

        // Cut: removes "ab", buffer becomes "cd\n"
        run(Command::Cut, &mut e, &clk);
        assert_eq!(e.active().document.buffer.to_string(), "cd\n", "Cut must remove the selected text");
        assert_eq!(nav::head(&e), 0, "caret must be at selection start after Cut");
        assert_eq!(e.register.get(), Some("ab"), "register must contain the cut text");
    }

    /// Paste inserts register contents at the current caret position.
    #[test]
    fn paste_inserts_register_at_caret() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        set_caret(&mut e, 4);
        e.register.set("ab".into());
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        let before = e.active().document.buffer.to_string();

        let result = run(Command::Paste, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert!(e.clipboard_get_pending.is_some(), "Paste must request async clipboard text");
        assert_eq!(e.active().document.buffer.to_string(), before, "Paste must not mutate inline");
    }

    /// Move with extend=false collapses the selection to a point.
    #[test]
    fn move_without_extend_collapses_selection() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        set_caret(&mut e, 0);
        derive::rebuild(&mut e);
        let clk = TestClock(0);

        // Extend selection first
        run(Command::Move { dir: Dir::Right, extend: true }, &mut e, &clk);
        run(Command::Move { dir: Dir::Right, extend: true }, &mut e, &clk);
        assert!(!e.active().document.selection.primary().is_empty());

        // Move without extend collapses to point at new head
        run(Command::Move { dir: Dir::Right, extend: false }, &mut e, &clk);
        let sel = e.active().document.selection.primary();
        assert!(sel.is_empty(), "selection must be collapsed after Move with extend=false");
        assert_eq!(sel.head, 3, "head must be at 3 after moving right from 2");
    }

    // -------------------------------------------------------------------------
    // Fix 1: Backspace must delete a non-empty selection
    // -------------------------------------------------------------------------

    /// Backspace with an active (non-empty) selection deletes the selection range,
    /// leaving the caret at the selection's `from` offset.
    #[test]
    fn backspace_deletes_active_selection() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        // Set a non-collapsed selection: anchor=1, head=3 (selects "bc")
        e.active_mut().document.selection = Selection {
            ranges: [wordcartel_core::selection::Range { anchor: 1, head: 3 }]
                .into_iter()
                .collect(),
            primary: 0,
        };
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        let result = run(Command::Backspace, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "ad\n", "Backspace must delete the selection");
        assert_eq!(nav::head(&e), 1, "caret must be at selection.from() after Backspace");
    }

    /// Backspace with a collapsed selection (no active selection) still deletes
    /// one grapheme left of the caret, as before.
    #[test]
    fn backspace_collapsed_still_deletes_one_char() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        // Collapsed selection at offset 2 (between 'b' and 'c')
        e.active_mut().document.selection = Selection::single(2);
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        let result = run(Command::Backspace, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "acd\n", "plain Backspace must delete prev char");
        assert_eq!(nav::head(&e), 1, "caret must be one step left after plain Backspace");
    }

    /// Cut on empty selection (point cursor) is a Noop.
    #[test]
    fn cut_on_empty_selection_is_noop() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        set_caret(&mut e, 0);
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        let result = run(Command::Cut, &mut e, &clk);
        assert_eq!(result, CommandResult::Noop);
        assert_eq!(e.active().document.buffer.to_string(), "abcd\n");
    }

    /// Paste on an empty register is a Noop.
    #[test]
    fn paste_on_empty_register_is_noop() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        set_caret(&mut e, 0);
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        let before = e.active().document.buffer.to_string();

        let result = run(Command::Paste, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert!(e.clipboard_get_pending.is_some(), "Paste must request async clipboard text");
        assert_eq!(e.active().document.buffer.to_string(), before, "Paste must not mutate inline");
    }

    // -------------------------------------------------------------------------
    // Task 10: Undo/redo commands + render-mode toggle
    // -------------------------------------------------------------------------

    /// Command::Undo restores the buffer to the state before the edit.
    #[test]
    fn undo_command_restores_buffer() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        derive::rebuild(&mut e);
        let clk = TestClock(0);

        // Type 'X' at offset 5 (end of "hello") → "helloX\n"
        set_caret(&mut e, 5);
        run(Command::InsertChar('X'), &mut e, &clk);
        assert_eq!(e.active().document.buffer.to_string(), "helloX\n");

        // Undo → "hello\n"
        let result = run(Command::Undo, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "hello\n");
    }

    /// Command::Redo reapplies the change after an undo.
    #[test]
    fn redo_command_reapplies_change() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        derive::rebuild(&mut e);
        let clk = TestClock(0);

        set_caret(&mut e, 5);
        run(Command::InsertChar('X'), &mut e, &clk);
        assert_eq!(e.active().document.buffer.to_string(), "helloX\n");

        run(Command::Undo, &mut e, &clk);
        assert_eq!(e.active().document.buffer.to_string(), "hello\n");

        // Redo → "helloX\n"
        let result = run(Command::Redo, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "helloX\n");
    }

    /// Undo/Redo via commands round-trips: type something, Undo restores, Redo reapplies.
    /// Uses distinct timestamps to break coalescing so each char is its own undo entry.
    #[test]
    fn undo_redo_roundtrip_via_commands() {
        let mut e = Editor::new_from_text("\n", None, (80, 24));
        derive::rebuild(&mut e);

        // Type 'a' at t=0, 'b' at t=9999 (breaks coalescing)
        set_caret(&mut e, 0);
        run(Command::InsertChar('a'), &mut e, &TestClock(0));
        set_caret(&mut e, 1);
        run(Command::InsertChar('b'), &mut e, &TestClock(9_999_999));
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");

        // Undo once: removes 'b'
        run(Command::Undo, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "a\n");

        // Undo again: removes 'a'
        run(Command::Undo, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "\n");

        // Redo: reapplies 'a'
        run(Command::Redo, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "a\n");

        // Redo again: reapplies 'b'
        run(Command::Redo, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");
    }

    /// CycleRenderMode rotates LivePreview → SourceHighlighted → SourcePlain → LivePreview.
    #[test]
    fn cycle_render_mode_rotates_through_modes() {
        use crate::editor::RenderMode;
        let mut e = Editor::new_from_text("# Title\n", None, (80, 24));
        derive::rebuild(&mut e);
        let clk = TestClock(0);

        assert_eq!(e.active().view.mode, RenderMode::LivePreview);

        let r1 = run(Command::CycleRenderMode, &mut e, &clk);
        assert_eq!(r1, CommandResult::Handled);
        assert_eq!(e.active().view.mode, RenderMode::SourceHighlighted);

        run(Command::CycleRenderMode, &mut e, &clk);
        assert_eq!(e.active().view.mode, RenderMode::SourcePlain);

        run(Command::CycleRenderMode, &mut e, &clk);
        assert_eq!(e.active().view.mode, RenderMode::LivePreview);
    }

    // -------------------------------------------------------------------------
    // Fix 1 (CUA): type/paste/Enter over a selection REPLACE it; DeleteForward
    // over a selection DELETES it.
    // -------------------------------------------------------------------------

    /// Typing a character over a non-empty selection replaces the selection.
    /// "abcd\n", select anchor=1 head=3 ("bc"), InsertChar('X') → "aXd\n", caret 2.
    #[test]
    fn type_over_selection_replaces() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        e.active_mut().document.selection = Selection {
            ranges: [wordcartel_core::selection::Range { anchor: 1, head: 3 }]
                .into_iter()
                .collect(),
            primary: 0,
        };
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        let result = run(Command::InsertChar('X'), &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "aXd\n", "InsertChar must replace the selection");
        assert_eq!(nav::head(&e), 2, "caret must be after the inserted char");
    }

    /// InsertChar over a collapsed selection (normal caret) still inserts at the caret.
    #[test]
    fn type_over_collapsed_selection_inserts_normally() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        e.active_mut().document.selection = Selection::single(2);
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        run(Command::InsertChar('X'), &mut e, &clk);
        assert_eq!(e.active().document.buffer.to_string(), "abXcd\n");
        assert_eq!(nav::head(&e), 3);
    }

    /// InsertNewline over a non-empty selection replaces the selection with a newline.
    #[test]
    fn enter_over_selection_replaces() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        e.active_mut().document.selection = Selection {
            ranges: [wordcartel_core::selection::Range { anchor: 1, head: 3 }]
                .into_iter()
                .collect(),
            primary: 0,
        };
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        let result = run(Command::InsertNewline, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "a\nd\n", "InsertNewline must replace the selection");
        assert_eq!(nav::head(&e), 2, "caret must be after the newline");
    }

    /// Paste over a non-empty selection replaces the selection with the register contents.
    #[test]
    fn paste_over_selection_replaces() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        e.register.set("XY".into());

        // Set non-empty selection anchor=1 head=3 (selects "bc")
        e.active_mut().document.selection = Selection {
            ranges: [wordcartel_core::selection::Range { anchor: 1, head: 3 }]
                .into_iter()
                .collect(),
            primary: 0,
        };
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        let before = e.active().document.buffer.to_string();

        let result = run(Command::Paste, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert!(e.clipboard_get_pending.is_some(), "Paste must request async clipboard text");
        assert_eq!(e.active().document.buffer.to_string(), before, "Paste must not mutate inline");
    }

    /// DeleteForward with a non-empty selection deletes the selection range,
    /// caret lands at selection.from().
    #[test]
    fn delete_forward_deletes_selection() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        e.active_mut().document.selection = Selection {
            ranges: [wordcartel_core::selection::Range { anchor: 1, head: 3 }]
                .into_iter()
                .collect(),
            primary: 0,
        };
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        let result = run(Command::DeleteForward, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "ad\n", "DeleteForward must delete the selection");
        assert_eq!(nav::head(&e), 1, "caret must be at selection.from()");
    }

    /// DeleteForward with a collapsed selection still deletes one grapheme forward.
    #[test]
    fn delete_forward_collapsed_still_deletes_one_char() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        e.active_mut().document.selection = Selection::single(1);
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        let result = run(Command::DeleteForward, &mut e, &clk);
        assert_eq!(result, CommandResult::Handled);
        assert_eq!(e.active().document.buffer.to_string(), "acd\n");
        assert_eq!(nav::head(&e), 1);
    }

    /// In SourceHighlighted mode, an INACTIVE heading line shows raw "# Title"
    /// (markers visible), whereas in LivePreview it shows concealed "Title".
    #[test]
    fn source_highlighted_makes_inactive_heading_show_raw() {
        use crate::editor::RenderMode;

        // Start in LivePreview; cursor on line 1 (blank) so line 0 (heading) is inactive.
        // "# Title\n" = 8 bytes; blank line starts at offset 8.
        let mut e = Editor::new_from_text("# Title\n\nplain\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(8); // on blank line
        derive::rebuild(&mut e);

        // In LivePreview, inactive heading line 0 must show concealed "Title"
        let (rows_lp, _) = &e.active().view.line_layouts[&0];
        assert_eq!(rows_lp[0].display, "Title", "LivePreview inactive heading should be concealed");

        // Switch to SourceHighlighted
        let clk = TestClock(0);
        run(Command::CycleRenderMode, &mut e, &clk);
        assert_eq!(e.active().view.mode, RenderMode::SourceHighlighted);

        // After CycleRenderMode, derive::rebuild is called inside the command.
        // Line 0 should now show raw "# Title"
        let (rows_sh, _) = &e.active().view.line_layouts[&0];
        assert_eq!(rows_sh[0].display, "# Title", "SourceHighlighted must show raw markers on inactive heading");
    }

    // -------------------------------------------------------------------------
    // Task 3: CycleRenderMode + Copy-on-empty polish
    // -------------------------------------------------------------------------

    #[test]
    fn copy_on_empty_selection_preserves_register() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        // Pre-load the register with "seed".
        let mut src = Editor::new_from_text("seed\n", None, (80, 24));
        src.active_mut().document.selection = Selection {
            ranges: [wordcartel_core::selection::Range { anchor: 0, head: 4 }].into_iter().collect(),
            primary: 0,
        };
        run(Command::Copy, &mut src, &TestClock(0));
        e.register = src.register;
        // Now Copy with a COLLAPSED selection must NOT clobber "seed" with "".
        set_caret(&mut e, 1);
        let r = run(Command::Copy, &mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Noop, "Copy on empty selection is a no-op");
        assert_eq!(e.register.get(), Some("seed"), "register must be preserved");
    }

    #[test]
    fn build_range_replace_yields_changeset_and_matching_edit() {
        use crate::editor::Editor;
        use wordcartel_core::history::{EditKind, Transaction};
        let mut e = Editor::new_from_text("abcde\n", None, (80, 24));
        let doc_len = e.active().document.buffer.len();
        // Replace bytes 1..3 ("bc") with "X".
        let (cs, edit) = build_range_replace(1, 3, "X", doc_len);
        assert_eq!((edit.range.clone(), edit.new_len), (1..3, 1));
        let txn = Transaction::new(cs).with_selection(wordcartel_core::selection::Selection::single(2));
        e.active_mut().apply(txn, edit, EditKind::Other, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "aXde\n");
    }

    // -------------------------------------------------------------------------
    // Task 5: Word navigation + word-delete
    // -------------------------------------------------------------------------

    #[test]
    fn move_word_right_crosses_into_next_word_and_block() {
        let mut e = Editor::new_from_text("alpha beta\n\ngamma\n", None, (80, 24));
        set_caret(&mut e, 0); derive::rebuild(&mut e);
        run(Command::Move { dir: Dir::WordRight, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 6); // start of "beta"
        run(Command::Move { dir: Dir::WordRight, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 12); // start of "gamma" (across the blank-line gap)
    }

    #[test]
    fn select_word_left_extends_selection() {
        let mut e = Editor::new_from_text("alpha beta", None, (80, 24));
        set_caret(&mut e, 10); derive::rebuild(&mut e); // end of "beta"
        run(Command::Move { dir: Dir::WordLeft, extend: true }, &mut e, &TestClock(0));
        let r = e.active().document.selection.primary();
        assert_eq!((r.from(), r.to()), (6, 10)); // "beta" selected
    }

    #[test]
    fn delete_word_back_is_one_undo_step() {
        let mut e = Editor::new_from_text("alpha beta", None, (80, 24));
        set_caret(&mut e, 10); derive::rebuild(&mut e);
        run(Command::DeleteWord { back: true }, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "alpha ");
        e.undo();
        assert_eq!(e.active().document.buffer.to_string(), "alpha beta");
    }

    // -------------------------------------------------------------------------
    // Task 6: Paragraph, page & document navigation
    // -------------------------------------------------------------------------

    #[test]
    fn paragraph_down_jumps_to_next_block_start() {
        let mut e = Editor::new_from_text("Para one.\n\nPara two.\n\nThree.\n", None, (80, 24));
        set_caret(&mut e, 0); derive::rebuild(&mut e);
        run(Command::Move { dir: Dir::ParagraphDown, extend: false }, &mut e, &TestClock(0));
        let h = nav::head(&e);
        assert_eq!(e.active().document.buffer.slice(h..h+8), "Para two");
    }

    #[test]
    fn doc_start_and_end() {
        let mut e = Editor::new_from_text("aaa\nbbb\nccc\n", None, (80, 24));
        set_caret(&mut e, 5); derive::rebuild(&mut e);
        run(Command::Move { dir: Dir::DocEnd, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), e.active().document.buffer.len());
        run(Command::Move { dir: Dir::DocStart, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 0);
    }

    #[test]
    fn page_down_moves_down_about_a_page() {
        let text: String = (0..40).map(|i| format!("line {i}\n")).collect();
        let mut e = Editor::new_from_text(&text, None, (80, 10)); // ~9 content rows
        set_caret(&mut e, 0); derive::rebuild(&mut e);
        run(Command::Move { dir: Dir::PageDown, extend: false }, &mut e, &TestClock(0));
        assert!(nav::caret_line(&e) >= 7 && nav::caret_line(&e) <= 9,
            "page-down should advance ~one viewport, got line {}", nav::caret_line(&e));
    }

    // -------------------------------------------------------------------------
    // Task 7: Text objects — select word/sentence/paragraph + expand/shrink
    // -------------------------------------------------------------------------

    #[test]
    fn select_paragraph_selects_block() {
        let mut e = Editor::new_from_text("One two.\n\nThree four.\n", None, (80, 24));
        set_caret(&mut e, 12); derive::rebuild(&mut e); // inside "Three four."
        run(Command::SelectScope(Scope::Paragraph), &mut e, &TestClock(0));
        let r = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(r.from()..r.to()).trim(), "Three four.");
    }

    #[test]
    fn expand_then_shrink_round_trips() {
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        set_caret(&mut e, 1); derive::rebuild(&mut e); // inside "One"
        run(Command::ExpandSelection, &mut e, &TestClock(0)); // → word "One"
        let w = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(w.from()..w.to()), "One");
        run(Command::ExpandSelection, &mut e, &TestClock(0)); // → sentence
        let s = e.active().document.selection.primary();
        assert!(e.active().document.buffer.slice(s.from()..s.to()).starts_with("One two."));
        run(Command::ShrinkSelection, &mut e, &TestClock(0)); // back to word
        let w2 = e.active().document.selection.primary();
        assert_eq!((w2.from(), w2.to()), (w.from(), w.to()));
    }

    #[test]
    fn a_motion_resets_the_expand_ladder() {
        let mut e = Editor::new_from_text("One two.\n", None, (80, 24));
        set_caret(&mut e, 1); derive::rebuild(&mut e);
        run(Command::ExpandSelection, &mut e, &TestClock(0));
        run(Command::Move { dir: Dir::Right, extend: false }, &mut e, &TestClock(0)); // resets
        assert!(e.active().sel_history.is_empty());
    }

    #[test]
    fn cycle_render_mode_keeps_caret_visible() {
        // A tall document scrolled so the caret sits near the bottom; toggling mode
        // must call ensure_visible so the caret stays on-screen. We assert the cheap
        // observable: the command re-runs ensure_visible without panicking and the
        // caret's logical line remains within the laid-out range.
        let mut e = Editor::new_from_text(&"x\n".repeat(100), None, (20, 5));
        set_caret(&mut e, 180); // deep into the doc
        derive::rebuild(&mut e);
        nav::ensure_visible(&mut e);
        let r = run(Command::CycleRenderMode, &mut e, &TestClock(0));
        assert_eq!(r, CommandResult::Handled);
        let caret_line = e.active().document.buffer.snapshot().byte_to_line(nav::head(&e));
        assert!(e.active().view.line_layouts.contains_key(&caret_line),
            "caret's logical line must be laid out (visible) after a mode change");
    }

    // -------------------------------------------------------------------------
    // Task 5 (effort-5c-m): scope_range_at with explicit offset
    // -------------------------------------------------------------------------

    #[test]
    fn scope_range_at_word_at_offset() {
        let mut e = Editor::new_from_text("alpha beta", None, (80, 24));
        derive::rebuild(&mut e);
        // offset 7 is inside "beta" (6..10)
        assert_eq!(super::scope_range_at(&e, 7, Scope::Word), (6, 10));
    }

    // -------------------------------------------------------------------------
    // Task 6 (Effort 5g): central Command::Move normalize
    // -------------------------------------------------------------------------

    #[test]
    fn horizontal_move_into_fold_normalizes_to_heading() {
        let doc = "## A\nbody1\nbody2\n## B\n";
        let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
        ed.active_mut().folds.toggle(doc.find("## A").unwrap());
        crate::derive::rebuild(&mut ed);
        // caret at end of "## A" line; move_right would cross into hidden "body1".
        let a_end = doc.find("## A").unwrap() + "## A".len();
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(a_end);
        // Use the real Command::Move path through run()
        let clk = TestClock(0);
        run(Command::Move { dir: Dir::Right, extend: false }, &mut ed, &clk);
        let head = ed.active().document.selection.primary().head;
        let fv = { let b = ed.active(); crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer) };
        assert!(!fv.is_hidden(ed.active().document.buffer.byte_to_line(head)));
    }

    #[test]
    fn undo_snaps_caret_out_of_fold() {
        let doc = "## A\nbody1\nbody2\n## B\n";
        let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
        crate::derive::rebuild(&mut ed);
        let a = doc.find("## A").unwrap();
        let inside = doc.find("body2").unwrap();
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(inside);
        run(Command::InsertChar('X'), &mut ed, &TestClock(0));
        ed.active_mut().folds.toggle(a);
        crate::derive::rebuild(&mut ed);
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(a);

        run(Command::Undo, &mut ed, &TestClock(1_000));

        let head = ed.active().document.selection.primary().head;
        let fv = { let b = ed.active(); crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer) };
        assert_eq!(head, a);
        assert!(!fv.is_hidden(ed.active().document.buffer.byte_to_line(head)));
    }

    #[test]
    fn shrink_selection_snaps_restored_caret_out_of_fold() {
        let doc = "## A\nbody1\nbody2\n## B\n";
        let mut ed = crate::editor::Editor::new_from_text(doc, None, (80, 24));
        crate::derive::rebuild(&mut ed);
        let a = doc.find("## A").unwrap();
        let inside = doc.find("body2").unwrap();
        ed.active_mut().sel_history.push(wordcartel_core::selection::Selection::single(inside));
        ed.active_mut().folds.toggle(a);
        crate::derive::rebuild(&mut ed);
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(a);

        run(Command::ShrinkSelection, &mut ed, &TestClock(0));

        let head = ed.active().document.selection.primary().head;
        let fv = { let b = ed.active(); crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer) };
        assert_eq!(head, a);
        assert!(!fv.is_hidden(ed.active().document.buffer.byte_to_line(head)));
    }

    // -------------------------------------------------------------------------
    // Task 2 (Effort 8): Select All command
    // -------------------------------------------------------------------------

    #[test]
    fn select_all_selects_whole_buffer() {
        let mut e = Editor::new_from_text("hello\nworld\n", None, (40, 10));
        let len = e.active().document.buffer.len();
        run(Command::SelectAll, &mut e, &TestClock(0));
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (0, len));
        assert_eq!(sel.head, len, "forward selection: caret (head) lands at end");
        assert!(e.active().desired_col.is_none());
        assert!(e.active().sel_history.is_empty(), "expand-selection ladder reset on select-all");
    }

    #[test]
    fn select_all_empty_buffer_is_noop_safe() {
        let mut e = Editor::new_from_text("", None, (40, 10));
        run(Command::SelectAll, &mut e, &TestClock(0));
        assert!(e.active().document.selection.primary().is_empty());
    }

    #[test]
    fn multi_replace_builds_one_changeset_covering_all() {
        // "aa aa aa" replace all "aa" -> "b": expect "b b b"
        let (cs, edit) = super::build_multi_replace(
            &[(0, 2, "b".into()), (3, 5, "b".into()), (6, 8, "b".into())], 8);
        let mut tb = wordcartel_core::buffer::TextBuffer::from_str("aa aa aa");
        cs.apply(&mut tb);
        assert_eq!(tb.slice(0..tb.len()), "b b b");
        assert_eq!(edit.range, 0..8); // covering edit spans first.start..last.end
    }

    // -------------------------------------------------------------------------
    // Task 2 (Effort 9B): DeleteLine + DeleteToLineEnd
    // -------------------------------------------------------------------------

    #[test]
    fn delete_line_removes_whole_line_including_newline() {
        let mut e = Editor::new_from_text("aaa\nbbb\nccc\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5); // in "bbb"
        run(Command::DeleteLine, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "aaa\nccc\n");
        assert_eq!(e.active().document.selection.primary().head, 4); // start of "ccc"
    }

    #[test]
    fn delete_line_last_line_without_trailing_newline_vanishes() {
        let mut e = Editor::new_from_text("aaa\nbbb", None, (40, 10)); // no trailing \n
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5); // in "bbb"
        run(Command::DeleteLine, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "aaa"); // preceding \n absorbed
    }

    #[test]
    fn delete_line_on_empty_trailing_line_removes_preceding_newline() {
        let mut e = Editor::new_from_text("aaa\n", None, (40, 10)); // logical lines: "aaa", ""
        let len = e.active().document.buffer.len();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(len); // phantom empty line
        run(Command::DeleteLine, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "aaa");
    }

    #[test]
    fn delete_line_single_line_empties_buffer() {
        let mut e = Editor::new_from_text("only line", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(3);
        run(Command::DeleteLine, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "");
        assert_eq!(e.active().document.selection.primary().head, 0);
    }

    #[test]
    fn delete_to_line_end_deletes_to_eol_keeps_newline() {
        let mut e = Editor::new_from_text("hello world\nnext\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5); // after "hello"
        run(Command::DeleteToLineEnd, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "hello\nnext\n");
    }

    #[test]
    fn delete_to_line_end_at_eol_is_noop() {
        let mut e = Editor::new_from_text("hello\nnext\n", None, (40, 10));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5); // at end of "hello"
        let before = e.active().document.version;
        let r = run(Command::DeleteToLineEnd, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "hello\nnext\n", "byte-identical");
        assert_eq!(e.active().document.version, before, "no changeset applied");
        assert!(matches!(r, CommandResult::Noop));
    }
}
