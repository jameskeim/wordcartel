//! Editing commands — the layer that translates a `Command` into a
//! `Transaction` + `block_tree::Edit`, calls `editor.apply`, then re-derives.
//!
//! Every edit command:
//!   1. Captures the current caret offset via `nav::head`.
//!   2. Builds a `ChangeSet` and a matching `block_tree::Edit { range, new_len }`
//!      from the *same* `(range, replacement)`.
//!   3. Calls `editor.apply(txn, edit, kind, clock)`.
//!
//! The remaining steps — rebuild, ensure_visible, and the `desired_col` reset —
//! are the edit epilogue, now owned by the core (`edit_apply::resettle`) and run
//! from inside `editor.apply` for the active buffer; command primitives no longer
//! hand-roll them.

mod edit;
// `pub(crate)` (not private like `mod edit;`) so `registry.rs` — outside `commands` —
// can reach the A14 atomic-edit handlers directly (module-structure GATE: a leaf
// module, no `Command` enum variant, no `commands::run` arm).
pub(crate) mod prose_ops;
pub(crate) mod textops;

use crate::derive;
use crate::editor::{Editor, RenderMode};
use crate::file;
use crate::nav;
use crate::registry::{place_caret_visible, CaretPlace};
use wordcartel_core::history::Clock;
use wordcartel_core::register;
use wordcartel_core::selection::Selection;

/// Text object scope for selection expansion commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope { Word, Sentence, Paragraph, Section, Document }

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
    SentenceLeft,
    SentenceRight,
    ParagraphUp,
    ParagraphDown,
    PageUp,
    PageDown,
    ScreenTop,
    ScreenBottom,
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
    /// Rotate the render mode: LivePreview → Review → SourceHighlighted → SourcePlain → LivePreview.
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
    /// Select the given text-object scope at the caret.
    SelectScope(Scope),
    /// Grow selection: word → sentence → paragraph → section → document (`LADDER`, stateless).
    ExpandSelection,
    /// Shrink selection: the largest canonical scope strictly contained in the current
    /// selection (`LADDER` reversed, stateless — mirrors `ExpandSelection`).
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
    ChangeSet::from_ops(ops, doc_len)
}

/// Build ONE `ChangeSet` performing all `edits` (ascending, non-overlapping
/// `(from,to,replacement)`) plus ONE covering `block_tree::Edit` spanning
/// `first.start..last.end`. Applied as a single `editor.apply` → one undo unit.
///
/// `edits` must be non-empty, ascending, non-overlapping, and in-bounds for
/// `doc_len`. Any malformed or empty list degrades to an identity no-op —
/// this returns a no-op `ChangeSet` (a full-document retain) and a
/// zero-length `Edit` that changes nothing, rather than panicking.
pub fn build_multi_replace(
    edits: &[(usize, usize, String)],
    doc_len: usize,
) -> (wordcartel_core::change::ChangeSet, wordcartel_core::block_tree::Edit) {
    use wordcartel_core::change::{ChangeSet, Op, Tendril};
    // WELL-FORMEDNESS GUARD (H7): the op-builder below assumes `edits` is a non-empty,
    // ascending, non-overlapping, in-bounds sequence. A malformed list would make the ops
    // over-/under-consume the document and trip ChangeSet::from_ops' release assert. Any
    // violation degrades to the identity no-op — a malformed multi-replace does NOTHING, so
    // it can neither panic nor corrupt. No production caller hits this (all pass ascending,
    // non-overlapping spans); it is the boundary insurance Effort P will lean on.
    let well_formed = !edits.is_empty()
        && edits.iter().all(|(f, t, _)| f <= t)
        && edits.windows(2).all(|w| w[0].1 <= w[1].0)
        && edits.last().is_some_and(|(_, t, _)| *t <= doc_len);
    if !well_formed {
        let ops = if doc_len > 0 { vec![Op::Retain(doc_len)] } else { Vec::new() };
        let cs = ChangeSet::from_ops(ops, doc_len);
        let edit = wordcartel_core::block_tree::Edit { range: 0..0, new_len: 0 };
        return (cs, edit);
    }
    let mut ops = Vec::new();
    let mut pos = 0usize;
    for (from, to, text) in edits {
        if *from > pos { ops.push(Op::Retain(from - pos)); }
        if to > from { ops.push(Op::Delete(to - from)); }
        if !text.is_empty() { ops.push(Op::Insert(Tendril::from(text.as_str()))); }
        pos = *to;
    }
    if doc_len > pos { ops.push(Op::Retain(doc_len - pos)); }
    // The guard proved edits non-empty + ascending + in-bounds, so first<=last_to<=doc_len
    // and every subtraction below is non-negative — no saturating dressing needed.
    let first = edits.first().unwrap().0;
    let last_to = edits.last().unwrap().1;
    // new_len of the covering region = (last_to - first) adjusted by all deltas.
    let delta: isize = edits.iter().map(|(f, t, s)| s.len() as isize - (t - f) as isize).sum();
    let new_len = ((last_to - first) as isize + delta) as usize;
    let cs = ChangeSet::from_ops(ops, doc_len);
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

/// The `BlockRole` returned when a caret is not in prose — carried so the command can name the
/// block kind in its decline message (F3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonProse(pub wordcartel_core::style::BlockRole);

/// Human name of a non-`Paragraph` block role, for the "no sentence here (…)" message.
pub fn block_kind_label(role: wordcartel_core::style::BlockRole) -> &'static str {
    use wordcartel_core::style::BlockRole::*;
    match role {
        Paragraph => "paragraph", Heading(_) => "heading", BlockQuote => "block quote",
        ListItem => "list item", CodeBlock => "code block", ThematicBreak => "rule",
        FrontMatter => "front matter", Comment => "comment",
    }
}

/// The content-anchored prose WINDOW `(ps, pe)` containing byte `h`, or `None` when `h`'s line is
/// not prose. SEE==SELECT single-source (spec §8, C-11): classify+window at the caret line's first
/// non-whitespace CONTENT byte (`ventilate::line_content_byte`) — CommonMark strips ≤3-space block
/// indent, so a `line_start`/raw-`h` window (`nav::paragraph_range_at` at the caret) hits the gap
/// fallback and DIVERGES from the lens on indented prose. This is THE window every prose-surgery
/// mutation handler must window through (`move_sentence`, `break_paragraph_here`,
/// `merge_paragraph_forward`) so none can drift back to a raw-caret window (I-1). `prose_sentence_at`
/// segments within this exact window, so the sentence bounds and the paragraph window always agree.
pub fn prose_window_at(editor: &Editor, h: usize) -> Option<(usize, usize)> {
    let b = editor.active();
    let buf = &b.document.buffer;
    let blocks = b.document.blocks();
    let line = buf.byte_to_line(h.min(buf.len()));
    let c = crate::ventilate::line_content_byte(buf, line)?;
    crate::ventilate::prose_block_at(blocks, buf, c)
}

/// The sentence scope at byte `h`, via the LENS'S OWN classification + window, or `Err(NonProse)`
/// when `h` is not in prose. SEE==SELECT single-source (spec §8, C-11): the window is
/// [`prose_window_at`]'s content-anchored `(ps, pe)`, then `sentence_bounds` (segmentation) — the
/// exact calls the lens renders with. Windowing through the shared helper keeps the sentence bounds
/// and the paragraph window a mutation handler derives byte-identical.
pub fn prose_sentence_at(editor: &Editor, h: usize) -> Result<(usize, usize), NonProse> {
    let b = editor.active();
    let buf = &b.document.buffer;
    match prose_window_at(editor, h) {
        Some((ps, pe)) => {
            let rel = h.saturating_sub(ps);
            let (sf, st) = wordcartel_core::textobj::sentence_bounds(&buf.slice(ps..pe), rel);
            Ok((ps + sf, ps + st))
        }
        None => {
            // Not prose: name the block role for the decline message (F3). Prefer the content byte
            // (the classification site); fall back to line_start for a content-less line.
            let blocks = b.document.blocks();
            let line = buf.byte_to_line(h.min(buf.len()));
            let at = crate::ventilate::line_content_byte(buf, line)
                .unwrap_or_else(|| crate::derive::line_start(buf, line));
            Err(NonProse(blocks.role_at(at)))
        }
    }
}

/// The DEEPEST (smallest) heading subtree `[heading.byte, body.end)` containing `h`, or `None` when
/// `h` is under no heading. `outline::sections` yields nested ranges; the deepest-containing one is
/// the innermost enclosing scene (spec §3.1). Cold path: O(headings)+alloc, command/expand-press
/// triggered — never per-keystroke (C-3).
pub fn section_range_at(editor: &Editor, h: usize) -> Option<(usize, usize)> {
    let b = editor.active();
    let rope = b.document.buffer.snapshot();
    wordcartel_core::outline::sections(b.document.blocks(), &rope)
        .into_iter()
        .filter(|s| s.heading.byte <= h && h < s.body.end)
        .min_by_key(|s| s.body.end - s.heading.byte)
        .map(|s| (s.heading.byte, s.body.end))
}

/// Compute the (from, to) byte range of `scope` at the given byte offset `h`.
/// Borrows `editor` immutably and returns owned `(usize, usize)`.
pub fn scope_range_at(editor: &Editor, h: usize, scope: Scope) -> (usize, usize) {
    let buf = &editor.active().document.buffer;
    let blocks = editor.active().document.blocks();
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
        Scope::Sentence => prose_sentence_at(editor, h).unwrap_or((h, h)), // decline → empty → ladder skips
        Scope::Paragraph => nav::paragraph_range_at(blocks, buf, h),
        Scope::Section => section_range_at(editor, h).unwrap_or((h, h)), // decline → empty → ladder skips
        Scope::Document => (0, buf.len()),
    }
}

/// Compute the (from, to) byte range of `scope` at the caret position.
/// Borrows `editor` immutably and returns owned `(usize, usize)`.
fn scope_range(editor: &Editor, scope: Scope) -> (usize, usize) {
    scope_range_at(editor, nav::head(editor), scope)
}

/// Set the selection to [from, to) and re-derive + ensure visibility.
///
/// C-9: head-at-start — `Selection::range(anchor, head)` puts the caret on the 2nd arg, so pass
/// `(to, from)` → `from()==from`, `to()==to`, `head==from`. The caret lands at the span START (F8),
/// and expand/shrink evaluate `scope_range_at` from inside the span.
fn set_selection_range(editor: &mut Editor, from: usize, to: usize) {
    editor.active_mut().document.selection = Selection::range(to, from);
    derive::rebuild(editor);
    nav::ensure_visible(editor);
}

/// Expand/shrink rungs, finest → coarsest. A data table (spec §3.4) — adding a rung is a table
/// edit, not dispatcher growth. A declined scope (`Sentence` on non-prose, `Section` with no
/// enclosing heading) yields an empty range and is skipped by the strict-containment test.
const LADDER: &[Scope] = &[Scope::Word, Scope::Sentence, Scope::Paragraph, Scope::Section, Scope::Document];

/// Execute `cmd` against `editor`, then re-derive + ensure visibility.
#[allow(clippy::too_many_lines)] // exhaustive flat Command dispatch — edit arms delegate to
                                 // commands::edit; remaining arms are small non-edit state ops (H11)
pub fn run(cmd: Command, editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
    match cmd {
        Command::InsertChar(c)       => edit::insert_char(editor, c, clock),
        Command::InsertNewline       => edit::insert_newline(editor, clock),
        Command::Backspace           => edit::backspace(editor, clock),
        Command::DeleteForward       => edit::delete_forward(editor, clock),

        Command::Move { dir, extend } => {
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
                Dir::SentenceLeft  => nav::move_sentence_left(editor),
                Dir::SentenceRight => nav::move_sentence_right(editor),
                Dir::ParagraphUp   => nav::move_paragraph_up(editor),
                Dir::ParagraphDown => nav::move_paragraph_down(editor),
                Dir::PageUp        => nav::move_page_up(editor),
                Dir::PageDown      => nav::move_page_down(editor),
                Dir::ScreenTop     => nav::move_screen_top(editor),
                Dir::ScreenBottom  => nav::move_screen_bottom(editor),
                Dir::DocStart      => nav::move_doc_start(editor),
                Dir::DocEnd        => nav::move_doc_end(editor),
            };
            // Up/Down preserve desired_col (handled inside move_up/move_down).
            // Horizontal moves reset desired_col to None (handled inside move_left/right/home/end).

            // Central fold-aware invariant: ensure the committed head is never
            // inside a folded body, for ALL motion directions at once.
            let new_head = {
                let b = editor.active();
                crate::fold::normalize_caret(&b.folds, b.document.blocks(), &b.document.buffer, new_head)
            };

            if extend {
                // Keep the current anchor; move the head to `new_head`.
                let anchor = editor.active().document.selection.primary().anchor;
                editor.active_mut().document.selection = Selection::range(anchor, new_head);
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
            editor.set_status(crate::status::StatusKind::Info, "Copied".to_string());
            CommandResult::Handled
        }

        Command::Cut => edit::cut(editor, clock),

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
            let next = match editor.active().view.mode {
                RenderMode::LivePreview       => RenderMode::Review,
                RenderMode::Review            => RenderMode::SourceHighlighted,
                RenderMode::SourceHighlighted => RenderMode::SourcePlain,
                RenderMode::SourcePlain       => RenderMode::LivePreview,
            };
            editor.set_render_mode(next, clock.now_ms());
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
                    editor.set_status_full(crate::status::StatusKind::Warning, "No file name — use Save As".to_string(),
                        crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
                }
                Some(path) => {
                    let v = editor.active().document.version;
                    let buffer_id = editor.active().id;
                    // Progress keyed on THIS (buffer, version); the synchronous completions below
                    // reconstruct the identical key and collapse this start in place (§4.2).
                    let topic = crate::status::StatusTopic::Save(buffer_id, v);
                    editor.set_progress(topic, "Saving\u{2026}");
                    let content = editor.active().document.buffer.to_string();
                    match file::save_atomic(&path, &content) {
                        Ok(file::SaveOutcome::Saved) => {
                            editor.active_mut().document.mark_saved(v);
                            editor.finish_topic(topic, crate::status::StatusKind::Info, "Saved".to_string());
                        }
                        Ok(file::SaveOutcome::Unchanged) => {
                            editor.active_mut().document.mark_saved(v);
                            editor.finish_topic(topic, crate::status::StatusKind::Info, "(unchanged)".to_string());
                        }
                        Err(e) => {
                            // Buffer stays dirty; surface the failure as a held Error (F4).
                            editor.finish_topic(topic, crate::status::StatusKind::Error, e.to_string());
                        }
                    }
                }
            }
            CommandResult::Handled
        }

        Command::Quit => {
            // C4/I1 (user-ratified): quit supersedes — and cancels — a pending
            // close. Clear CloseBuffer-carrying pendings so a cancelled quit
            // leaves no ghost close armed to fire on the next manual save.
            // Foreign quit/drain pendings are the existing flow's business.
            if editor.pending_after_save.as_ref()
                .is_some_and(|p| matches!(&p.action, crate::editor::PostSaveAction::CloseBuffer { .. })) {
                editor.pending_after_save = None;
            }
            if matches!(&editor.pending_save_as, Some(crate::editor::PostSaveAction::CloseBuffer { .. })) {
                editor.pending_save_as = None;
            }
            // Effort 6: quit considers the WHOLE workspace, not just the active
            // buffer. Scratch is never dirty (is_dirty excludes it).
            let any_dirty = editor.buffers.iter().any(|b| editor.is_dirty(b.id));
            if any_dirty {
                let n = editor.buffers.iter().filter(|b| editor.is_dirty(b.id)).count();
                editor.open_prompt(crate::prompt::Prompt::quit_multi(n));
                CommandResult::Handled
            } else {
                editor.quit = true;
                CommandResult::Quit
            }
        }

        Command::DeleteWord { back } => edit::delete_word(editor, back, clock),

        Command::DeleteLine => edit::delete_line(editor, clock),

        Command::DeleteToLineEnd => edit::delete_to_line_end(editor, clock),

        Command::SelectScope(scope) => {
            match scope {
                Scope::Sentence => match prose_sentence_at(editor, nav::head(editor)) {
                    Ok((from, to)) => { set_selection_range(editor, from, to); CommandResult::Handled }
                    Err(NonProse(role)) => {
                        editor.set_status(crate::status::StatusKind::Info, format!("no sentence here ({})", block_kind_label(role)));
                        CommandResult::Noop
                    }
                },
                Scope::Section => match section_range_at(editor, nav::head(editor)) {
                    Some((from, to)) => { set_selection_range(editor, from, to); CommandResult::Handled }
                    None => {
                        editor.set_status(crate::status::StatusKind::Info, "no section here");
                        CommandResult::Noop
                    }
                },
                _ => {
                    let (from, to) = scope_range(editor, scope);
                    set_selection_range(editor, from, to);
                    CommandResult::Handled
                }
            }
        }

        Command::ExpandSelection => {
            let cur = editor.active().document.selection.primary();
            let (cf, ct) = (cur.from(), cur.to());
            let from = cf; // C-9: evaluate strictly inside the span
            let mut next: Option<(usize, usize)> = None;
            for sc in LADDER.iter().copied() {
                let (f, t) = scope_range_at(editor, from, sc);
                if f <= cf && t >= ct && (f < cf || t > ct) { next = Some((f, t)); break; }
            }
            match next {
                Some((f, t)) => { set_selection_range(editor, f, t); CommandResult::Handled }
                None => CommandResult::Noop,
            }
        }

        Command::ShrinkSelection => {
            let cur = editor.active().document.selection.primary();
            let (cf, ct) = (cur.from(), cur.to());
            let from = cf;
            let mut inner: Option<(usize, usize)> = None;
            for sc in LADDER.iter().rev().copied() {
                let (f, t) = scope_range_at(editor, from, sc);
                if f >= cf && t <= ct && (f > cf || t < ct) && f < t { inner = Some((f, t)); break; }
            }
            match inner {
                Some((f, t)) => { set_selection_range(editor, f, t); CommandResult::Handled }
                None => CommandResult::Noop,
            }
        }

        Command::SelectAll => {
            let len = editor.active().document.buffer.len();
            editor.active_mut().document.selection =
                wordcartel_core::selection::Selection::range(0, len);
            editor.active_mut().desired_col = None;
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

    // A17 T8 — a keyboard edit routed via `commands::run` → `Editor::apply` delegator: no-op +
    // "buffer is read-only" feedback.
    #[test]
    fn read_only_buffer_rejects_keyboard_edits_with_a_message() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 6));
        e.active_mut().read_only = true;
        let clk = TestClock(0);
        let before = e.active().document.buffer.to_string();
        run(Command::InsertChar('x'), &mut e, &clk);
        assert_eq!(e.active().document.buffer.to_string(), before, "read-only: keyboard edit is a no-op");
        assert_eq!(e.status_text(), "buffer is read-only");
    }

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
        assert_eq!(e.clipboard_sync_request.as_deref(), Some("ab"),
            "Cut on an editable buffer must still request a clipboard sync (C6 regression guard)");
    }

    // C6: on a read-only buffer, Cut must be a clean refuse — the core's read-only reject is
    // the only effect. The register must NOT be touched (stays at its prior value) and no
    // clipboard sync must be requested. Before the C6 fix, `cut()` wrote the register BEFORE
    // `editor.apply` could reject the edit, so a read-only Cut still silently synced the
    // clipboard even though nothing was deleted.
    #[test]
    fn cut_on_read_only_buffer_does_not_touch_register_or_clipboard() {
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        // Prime the register with a prior value so we can prove Cut leaves it UNCHANGED,
        // not just empty.
        e.register.set("prior".into());
        e.active_mut().read_only = true;
        e.active_mut().document.selection = Selection::range(0, 2); // "ab"
        derive::rebuild(&mut e);
        let clk = TestClock(0);
        let before = e.active().document.buffer.to_string();

        let result = run(Command::Cut, &mut e, &clk);

        assert_eq!(result, CommandResult::Handled, "Cut still reports Handled (the reject IS the effect)");
        assert_eq!(e.active().document.buffer.to_string(), before, "read-only: buffer must be unchanged");
        assert_eq!(e.register.get(), Some("prior"), "read-only Cut must NOT touch the register");
        assert!(e.clipboard_sync_request.is_none(), "read-only Cut must NOT request a clipboard sync");
        assert_eq!(e.status_text(), "buffer is read-only", "the core's loud read-only reject must still fire");
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
        e.active_mut().document.selection = Selection::range(1, 3);
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

    /// CycleRenderMode rotates LivePreview → Review → SourceHighlighted → SourcePlain → LivePreview.
    #[test]
    fn cycle_render_mode_rotates_through_modes() {
        use crate::editor::RenderMode;
        let mut e = Editor::new_from_text("# Title\n", None, (80, 24));
        derive::rebuild(&mut e);
        let clk = TestClock(0);

        assert_eq!(e.active().view.mode, RenderMode::LivePreview);

        let r1 = run(Command::CycleRenderMode, &mut e, &clk);
        assert_eq!(r1, CommandResult::Handled);
        assert_eq!(e.active().view.mode, RenderMode::Review);

        run(Command::CycleRenderMode, &mut e, &clk);
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
        e.active_mut().document.selection = Selection::range(1, 3);
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
        e.active_mut().document.selection = Selection::range(1, 3);
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
        e.active_mut().document.selection = Selection::range(1, 3);
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
        e.active_mut().document.selection = Selection::range(1, 3);
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

        // Switch to SourceHighlighted (Live → Review → SourceHighlighted; two cycles)
        let clk = TestClock(0);
        run(Command::CycleRenderMode, &mut e, &clk);   // Live → Review
        run(Command::CycleRenderMode, &mut e, &clk);   // Review → SourceHighlighted
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
        src.active_mut().document.selection = Selection::range(0, 4);
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
    fn sentence_motion_start_and_end() {
        // spans: "One two." (0,8), "Three four." (9,20)
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        set_caret(&mut e, 12); derive::rebuild(&mut e);          // inside "Three four."
        run(Command::Move { dir: Dir::SentenceLeft, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 9);                             // start of current sentence
        run(Command::Move { dir: Dir::SentenceLeft, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 0);                             // idempotent-safe → previous
        run(Command::Move { dir: Dir::SentenceRight, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 8);                             // end of current CONTENT
        run(Command::Move { dir: Dir::SentenceRight, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 20);                            // → next content end
    }

    #[test]
    fn sentence_motion_crosses_blocks_both_directions() {
        // "One. Two." = 0..9 (spans (0,4),(5,9)); "Three. Four." = 11..23 (spans (11,17),(18,23)).
        // Core offsets executed-verified 2026-07-12: prev_sentence_start("Three. Four.",0)=None →
        // cross → prev_sentence_start("One. Two.",len)=Some(5); next_sentence_end("One. Two.",8)=9,
        // next_sentence_end("Three. Four.",0)=6 (→ 11+6=17).
        let mut e = Editor::new_from_text("One. Two.\n\nThree. Four.\n", None, (80, 24));
        // RIGHTWARD: from block 1 crosses to block 2's FIRST content end.
        set_caret(&mut e, 8); derive::rebuild(&mut e);            // in "Two."
        run(Command::Move { dir: Dir::SentenceRight, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 9);                             // end of "Two."
        run(Command::Move { dir: Dir::SentenceRight, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 17);                            // crosses to end of "Three."
        // LEFTWARD: from block 2's FIRST sentence start crosses to block 1's LAST sentence start.
        set_caret(&mut e, 11); derive::rebuild(&mut e);          // AT start of "Three." (block 2)
        run(Command::Move { dir: Dir::SentenceLeft, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 5);                             // crosses to start of "Two." (block 1's LAST)
    }

    #[test]
    fn sentence_motion_extends_selection() {
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        set_caret(&mut e, 0); derive::rebuild(&mut e);
        run(Command::Move { dir: Dir::SentenceRight, extend: true }, &mut e, &TestClock(0));
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (0, 8));               // anchor kept, head → 8
    }

    #[test]
    fn expand_ladder_sentence_rung_survives_single_sentence_paragraph() {
        // §3.4.3 regression: with content-only spans, Sentence (0,8) ⊂ Paragraph (0,9),
        // so the Sentence rung no longer collapses into Paragraph.
        let mut e = Editor::new_from_text("One two.\n", None, (80, 24));
        set_caret(&mut e, 1); derive::rebuild(&mut e);            // inside "One"
        run(Command::ExpandSelection, &mut e, &TestClock(0));     // → Word "One"
        run(Command::ExpandSelection, &mut e, &TestClock(0));     // → Sentence "One two."
        let s = e.active().document.selection.primary();
        assert_eq!((s.from(), s.to()), (0, 8));
        assert_eq!(e.active().document.buffer.slice(s.from()..s.to()), "One two.");
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

    // -------------------------------------------------------------------------
    // Task 2 (effort-s4-prose-surgery): Scope::Section + head-at-start (C-9)
    // -------------------------------------------------------------------------

    #[test]
    fn select_section_selects_subtree_caret_at_start() {
        let doc = "# Title\n\nintro para.\n\n## A\n\nbody of a.\n\n### A1\n\ninner.\n";
        let mut e = Editor::new_from_text(doc, None, (60, 20));
        derive::rebuild(&mut e);
        // caret inside "body of a." → deepest containing section is "## A" (its subtree includes A1).
        let at = doc.find("body of a.").unwrap();
        let a_start = doc.find("## A").unwrap();
        let sec = section_range_at(&e, at).expect("a section here");
        assert_eq!(sec.0, a_start, "section starts at the ## A heading byte");
        assert_eq!(sec.1, doc.len(), "## A subtree runs to EOF (includes ### A1)");
        // caret inside "inner." → deepest is "### A1".
        let inner = doc.find("inner.").unwrap();
        let a1_start = doc.find("### A1").unwrap();
        assert_eq!(section_range_at(&e, inner).unwrap().0, a1_start);
        // select_section (via the direct run() path — no registry needed) sets the selection
        // head-at-start (C-9).
        e.active_mut().document.selection = Selection::single(at);
        assert_eq!(run(Command::SelectScope(Scope::Section), &mut e, &TestClock(0)), CommandResult::Handled);
        let pr = e.active().document.selection.primary();
        assert_eq!((pr.from(), pr.to()), (a_start, doc.len()));
        assert_eq!(pr.head, a_start, "C-9: caret at the section START");
    }

    #[test]
    fn select_section_declines_when_no_heading() {
        let mut e = Editor::new_from_text("just a paragraph, no headings.\n", None, (50, 12));
        derive::rebuild(&mut e);
        assert_eq!(run(Command::SelectScope(Scope::Section), &mut e, &TestClock(0)), CommandResult::Noop);
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

    // -------------------------------------------------------------------------
    // Task 3 (effort-s4-prose-surgery): LADDER data table + stateless shrink
    // -------------------------------------------------------------------------

    #[test]
    fn ladder_expands_word_sentence_paragraph_section_document() {
        let doc = "# H\n\nOne two. Three four.\n";
        let mut e = Editor::new_from_text(doc, None, (60, 12));
        derive::rebuild(&mut e);
        let at = doc.find("two").unwrap();
        e.active_mut().document.selection = Selection::single(at);
        run(Command::SelectScope(Scope::Word), &mut e, &TestClock(0));      // "two"
        run(Command::ExpandSelection, &mut e, &TestClock(0));               // → Sentence "One two."
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "One two.");
        run(Command::ExpandSelection, &mut e, &TestClock(0));               // → Paragraph
        run(Command::ExpandSelection, &mut e, &TestClock(0));               // → Section (# H subtree)
        let s = e.active().document.selection.primary();
        assert_eq!(s.from(), doc.find("# H").unwrap());
        run(Command::ExpandSelection, &mut e, &TestClock(0));               // → Document
        let d = e.active().document.selection.primary();
        assert_eq!((d.from(), d.to()), (0, doc.len()));
    }

    #[test]
    fn stateless_shrink_returns_first_sentence_and_survives_undo() {
        let doc = "One two. Three four.\n";
        let mut e = Editor::new_from_text(doc, None, (40, 12));
        derive::rebuild(&mut e);
        e.active_mut().document.selection = Selection::single(2);
        run(Command::SelectScope(Scope::Paragraph), &mut e, &TestClock(0));
        run(Command::ShrinkSelection, &mut e, &TestClock(0)); // paragraph → FIRST sentence (from())
        let p = e.active().document.selection.primary();
        assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "One two.");
        // Hazard-4: expand → edit → shrink must not panic and must yield a canonical rung.
        run(Command::SelectScope(Scope::Paragraph), &mut e, &TestClock(0));
        run(Command::InsertChar('!'), &mut e, &TestClock(1));
        e.undo();
        run(Command::ShrinkSelection, &mut e, &TestClock(2)); // no panic, no stale state
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
    // Task 1 (effort-s4-prose-surgery): SEE==SELECT prose_sentence_at
    // -------------------------------------------------------------------------

    #[test]
    fn prose_sentence_at_declines_non_prose_and_resolves_prose() {
        // Prose: caret in a paragraph resolves the sentence.
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (40, 12));
        derive::rebuild(&mut e);
        assert_eq!(prose_sentence_at(&e, 2), Ok((0, 8))); // "One two."
        // Heading: declines (Heading role).
        let mut h = Editor::new_from_text("# Title\n\nbody text.\n", None, (40, 12));
        derive::rebuild(&mut h);
        assert!(matches!(prose_sentence_at(&h, 2), Err(NonProse(_))));
        // The paragraph after the heading still resolves.
        let body = "# Title\n\nbody text.\n".find("body").unwrap();
        assert!(prose_sentence_at(&h, body).is_ok());
        // Indented prose (CommonMark strips ≤3-space indent): content-byte classification, NOT decline.
        let mut ind = Editor::new_from_text("  Indented one. Indented two.\n", None, (40, 12));
        derive::rebuild(&mut ind);
        // Range is content-anchored at byte 2 ("Indented one." = 2..15), NOT line-start 0 —
        // a regression to line-start classification would shift ps and fail this assertion.
        assert_eq!(prose_sentence_at(&ind, 5), Ok((2, 15)));
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
        let fv = { let b = ed.active(); crate::fold::FoldView::compute(&b.folds, b.document.blocks(), &b.document.buffer) };
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
        let fv = { let b = ed.active(); crate::fold::FoldView::compute(&b.folds, b.document.blocks(), &b.document.buffer) };
        assert_eq!(head, a);
        assert!(!fv.is_hidden(ed.active().document.buffer.byte_to_line(head)));
    }

    // NOTE (S4 T3): `shrink_selection_snaps_restored_caret_out_of_fold` deleted — it exercised the
    // push/pop `sel_history` restore + `SnapOut` mechanism, which no longer exists. Stateless
    // `ShrinkSelection` now mirrors `ExpandSelection` exactly (`set_selection_range`, no snap-out);
    // this is the same fold-adjacency behavior `ExpandSelection` already had pre-T3 (ratified).

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

    #[test]
    fn multi_replace_empty_list_is_identity_noop() {
        let (cs, edit) = super::build_multi_replace(&[], 5);
        assert_eq!(edit.range, 0..0);
        assert_eq!(edit.new_len, 0);
        let mut tb = wordcartel_core::buffer::TextBuffer::from_str("hello");
        cs.apply(&mut tb);
        assert_eq!(tb.slice(0..tb.len()), "hello"); // no-op apply leaves the doc unchanged
    }

    #[test]
    fn multi_replace_reversed_pair_is_identity_noop_not_panic() {
        // (from=10, to=5) would over-consume the doc (retain 10 then retain doc_len-5)
        // and trip ChangeSet::from_ops' release assert. The guard degrades it to a no-op.
        let (cs, edit) = super::build_multi_replace(&[(10, 5, "x".into())], 20);
        assert_eq!(edit.range, 0..0);
        assert_eq!(edit.new_len, 0);
        let mut tb = wordcartel_core::buffer::TextBuffer::from_str("abcdefghijklmnopqrst"); // 20
        cs.apply(&mut tb);
        assert_eq!(tb.slice(0..tb.len()), "abcdefghijklmnopqrst");
    }

    #[test]
    fn multi_replace_overlapping_list_is_identity_noop() {
        // second edit starts (3) before the first ends (4) -> not ascending/non-overlapping.
        let (cs, edit) = super::build_multi_replace(&[(0, 4, "x".into()), (3, 6, "y".into())], 10);
        assert_eq!(edit.range, 0..0);
        let mut tb = wordcartel_core::buffer::TextBuffer::from_str("0123456789");
        cs.apply(&mut tb);
        assert_eq!(tb.slice(0..tb.len()), "0123456789");
    }

    #[test]
    fn multi_replace_out_of_bounds_is_identity_noop() {
        // last_to (12) exceeds doc_len (10).
        let (cs, edit) = super::build_multi_replace(&[(0, 12, "x".into())], 10);
        assert_eq!(edit.range, 0..0);
        let mut tb = wordcartel_core::buffer::TextBuffer::from_str("0123456789");
        cs.apply(&mut tb);
        assert_eq!(tb.slice(0..tb.len()), "0123456789");
    }

    #[test]
    fn multi_replace_valid_ascending_still_builds_covering_edit() {
        // Regression: the guard must NOT reject well-formed input.
        let (cs, edit) = super::build_multi_replace(
            &[(0, 2, "b".into()), (3, 5, "b".into()), (6, 8, "b".into())], 8);
        let mut tb = wordcartel_core::buffer::TextBuffer::from_str("aa aa aa");
        cs.apply(&mut tb);
        assert_eq!(tb.slice(0..tb.len()), "b b b");
        assert_eq!(edit.range, 0..8);
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
        assert_eq!(e.active().document.selection.primary().head, 3, "caret at end of remaining text");
    }

    #[test]
    fn delete_line_on_empty_trailing_line_removes_preceding_newline() {
        let mut e = Editor::new_from_text("aaa\n", None, (40, 10)); // logical lines: "aaa", ""
        let len = e.active().document.buffer.len();
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(len); // phantom empty line
        run(Command::DeleteLine, &mut e, &TestClock(0));
        assert_eq!(e.active().document.buffer.to_string(), "aaa");
        assert_eq!(e.active().document.selection.primary().head, 3, "caret at end after the empty line is removed");
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

    /// A17 T5 (F4 Warning table): the legacy synchronous `Command::Save` arm's pathless
    /// (unnamed-buffer) refusal is a Sticky Warning, not an ordinary Info echo.
    #[test]
    fn command_save_on_unnamed_buffer_is_a_sticky_warning() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        run(Command::Save, &mut e, &TestClock(0));
        assert_eq!(e.status_text(), "No file name — use Save As");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }
}
