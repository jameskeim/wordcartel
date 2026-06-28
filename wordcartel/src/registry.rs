//! Name-keyed command registry (spec §4.4 / §10.4). key → CommandId → Handler.
//! Built-in handlers delegate to the proven `commands::run` implementations so
//! the closed `Command` enum is shared built-in *implementation*, not the
//! dispatch boundary. Plugins (Effort P) register CommandId→Handler here without
//! touching the enum.

use std::collections::HashMap;

use crate::commands::{self, Command, CommandResult, Dir, Scope};
use crate::editor::Editor;
use crate::jobs::Executor;
use crate::app::Msg;
use wordcartel_core::history::Clock;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CommandId(pub &'static str);

impl std::borrow::Borrow<str> for CommandId {
    fn borrow(&self) -> &str {
        self.0
    }
}

/// Everything a handler may touch. The executor is here so job-dispatching
/// handlers (save, swap) have it; today's built-ins ignore it.
pub struct Ctx<'a> {
    pub editor: &'a mut Editor,
    pub clock: &'a dyn Clock,
    pub executor: &'a dyn Executor,
    /// Owned `Sender` (not a borrow) because `dispatch_filter` moves a clone into a `'static` spawned thread.
    pub msg_tx: std::sync::mpsc::Sender<Msg>,
}

pub type Handler = fn(&mut Ctx) -> CommandResult;

// ── Command metadata ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuCategory { File, Edit, Format, View, Export }

#[allow(dead_code)] // wired in Task 3/4
pub const MENU_ORDER: [MenuCategory; 5] =
    [MenuCategory::File, MenuCategory::Edit, MenuCategory::Format, MenuCategory::View, MenuCategory::Export];

#[derive(Clone, Copy)]
pub struct CommandMeta {
    pub label: &'static str,
    pub menu: Option<MenuCategory>,
}

// ── Registry ──────────────────────────────────────────────────────────────────

struct CommandEntry {
    id: CommandId,
    handler: Handler,
    meta: CommandMeta,
}

pub struct Registry {
    entries: Vec<CommandEntry>,
    index: HashMap<CommandId, usize>,
}

impl Registry {
    fn register(&mut self, id: &'static str, label: &'static str, menu: Option<MenuCategory>, handler: Handler) {
        let cid = CommandId(id);
        self.index.insert(cid, self.entries.len());
        self.entries.push(CommandEntry { id: cid, handler, meta: CommandMeta { label, menu } });
    }

    pub fn builtins() -> Registry {
        let mut r = Registry { entries: Vec::new(), index: HashMap::new() };

        // Motions (collapse selection) — palette-only (menu: None).
        r.register("move_left",       "Move Left",       None, |c| run(c, Command::Move { dir: Dir::Left,      extend: false }));
        r.register("move_right",      "Move Right",      None, |c| run(c, Command::Move { dir: Dir::Right,     extend: false }));
        r.register("move_up",         "Move Up",         None, |c| run(c, Command::Move { dir: Dir::Up,        extend: false }));
        r.register("move_down",       "Move Down",       None, |c| run(c, Command::Move { dir: Dir::Down,      extend: false }));
        r.register("move_line_start", "Move Line Start", None, |c| run(c, Command::Move { dir: Dir::LineStart, extend: false }));
        r.register("move_line_end",   "Move Line End",   None, |c| run(c, Command::Move { dir: Dir::LineEnd,   extend: false }));

        // Selecting motions (extend) — palette-only (menu: None).
        r.register("select_left",       "Select Left",       None, |c| run(c, Command::Move { dir: Dir::Left,      extend: true }));
        r.register("select_right",      "Select Right",      None, |c| run(c, Command::Move { dir: Dir::Right,     extend: true }));
        r.register("select_up",         "Select Up",         None, |c| run(c, Command::Move { dir: Dir::Up,        extend: true }));
        r.register("select_down",       "Select Down",       None, |c| run(c, Command::Move { dir: Dir::Down,      extend: true }));
        r.register("select_line_start", "Select Line Start", None, |c| run(c, Command::Move { dir: Dir::LineStart, extend: true }));
        r.register("select_line_end",   "Select Line End",   None, |c| run(c, Command::Move { dir: Dir::LineEnd,   extend: true }));

        // Word motions (collapse selection) — palette-only (menu: None).
        r.register("move_word_left",  "Move Word Left",  None, |c| run(c, Command::Move { dir: Dir::WordLeft,  extend: false }));
        r.register("move_word_right", "Move Word Right", None, |c| run(c, Command::Move { dir: Dir::WordRight, extend: false }));

        // Word selecting motions (extend) — palette-only (menu: None).
        r.register("select_word_left",  "Select Word Left",  None, |c| run(c, Command::Move { dir: Dir::WordLeft,  extend: true }));
        r.register("select_word_right", "Select Word Right", None, |c| run(c, Command::Move { dir: Dir::WordRight, extend: true }));

        // Paragraph / page / document navigation — palette-only (menu: None).
        r.register("move_paragraph_up",   "Move Paragraph Up",   None, |c| run(c, Command::Move { dir: Dir::ParagraphUp,   extend: false }));
        r.register("move_paragraph_down", "Move Paragraph Down", None, |c| run(c, Command::Move { dir: Dir::ParagraphDown, extend: false }));
        r.register("move_page_up",   "Move Page Up",   None, |c| run(c, Command::Move { dir: Dir::PageUp,   extend: false }));
        r.register("move_page_down", "Move Page Down", None, |c| run(c, Command::Move { dir: Dir::PageDown, extend: false }));
        r.register("move_doc_start", "Move to Start",  None, |c| run(c, Command::Move { dir: Dir::DocStart, extend: false }));
        r.register("move_doc_end",   "Move to End",    None, |c| run(c, Command::Move { dir: Dir::DocEnd,   extend: false }));

        // Word delete — Edit menu.
        r.register("delete_word_back",    "Delete Word Left",  Some(MenuCategory::Edit), |c| run(c, Command::DeleteWord { back: true }));
        r.register("delete_word_forward", "Delete Word Right", Some(MenuCategory::Edit), |c| run(c, Command::DeleteWord { back: false }));
        r.register("delete_line",         "Delete Line",        Some(MenuCategory::Edit), |c| run(c, Command::DeleteLine));
        r.register("delete_to_line_end",  "Delete to Line End", Some(MenuCategory::Edit), |c| run(c, Command::DeleteToLineEnd));

        // Editing — palette-only (menu: None).
        r.register("insert_newline", "Insert Newline",   None, |c| run(c, Command::InsertNewline));
        r.register("backspace",      "Backspace",        None, |c| run(c, Command::Backspace));
        r.register("delete_forward", "Delete Forward",   None, |c| run(c, Command::DeleteForward));

        // Edit menu.
        r.register("select_all", "Select All", Some(MenuCategory::Edit), |c| run(c, Command::SelectAll));
        r.register("copy",  "Copy",  Some(MenuCategory::Edit), |c| run(c, Command::Copy));
        r.register("cut",   "Cut",   Some(MenuCategory::Edit), |c| run(c, Command::Cut));
        r.register("paste", "Paste", Some(MenuCategory::Edit), |c| run(c, Command::Paste));
        r.register("undo",  "Undo",  Some(MenuCategory::Edit), |c| run(c, Command::Undo));
        r.register("redo",  "Redo",  Some(MenuCategory::Edit), |c| run(c, Command::Redo));
        r.register("filter", "Filter…", Some(MenuCategory::Edit), |c| {
            c.editor.open_minibuffer("> ", crate::minibuffer::MinibufferKind::Filter);
            CommandResult::Handled
        });
        r.register("find", "Find…", Some(MenuCategory::Edit), |c| {
            let origin = c.editor.active().document.selection.primary().to();
            c.editor.open_search(crate::search_overlay::Phase::Find, origin);
            CommandResult::Handled
        });
        r.register("replace", "Replace…", Some(MenuCategory::Edit), |c| {
            let origin = c.editor.active().document.selection.primary().to();
            c.editor.open_search(crate::search_overlay::Phase::Replace, origin);
            CommandResult::Handled
        });
        // find_next / find_prev are no-ops unless the overlay is open (handled in reduce);
        // register them so they appear in the palette and can be bound.
        r.register("find_next", "Find Next", None, |_c| CommandResult::Handled);
        r.register("find_prev", "Find Previous", None, |_c| CommandResult::Handled);

        // File menu.
        r.register("save", "Save", Some(MenuCategory::File), |c| crate::save::dispatch_save(c));
        r.register("quit", "Quit", Some(MenuCategory::File), |c| run(c, Command::Quit));

        // View menu.
        r.register("cycle_render_mode", "Cycle Render Mode", Some(MenuCategory::View), |c| run(c, Command::CycleRenderMode));
        r.register("transform", "Transform…", Some(MenuCategory::View), |c| {
            c.editor.open_prompt(crate::prompt::Prompt::transform_chooser());
            CommandResult::Handled
        });

        // Export menu.
        r.register("export_html", "Export HTML", Some(MenuCategory::Export), |c| {
            crate::export::run_export(c.editor, "html", &c.msg_tx);
            CommandResult::Handled
        });
        r.register("export_docx", "Export DOCX", Some(MenuCategory::Export), |c| {
            crate::export::run_export(c.editor, "docx", &c.msg_tx);
            CommandResult::Handled
        });
        r.register("export_pdf", "Export PDF", Some(MenuCategory::Export), |c| {
            crate::export::run_export(c.editor, "pdf", &c.msg_tx);
            CommandResult::Handled
        });

        // Text object selection — palette-only (Task 7 / Effort 5c).
        r.register("select_word",      "Select Word",      None, |c| run(c, Command::SelectScope(Scope::Word)));
        r.register("select_sentence",  "Select Sentence",  None, |c| run(c, Command::SelectScope(Scope::Sentence)));
        r.register("select_paragraph", "Select Paragraph", None, |c| run(c, Command::SelectScope(Scope::Paragraph)));
        r.register("expand_selection", "Expand Selection", None, |c| run(c, Command::ExpandSelection));
        r.register("shrink_selection", "Shrink Selection", None, |c| run(c, Command::ShrinkSelection));

        // View menu — palette command (Task 3 / Effort 5b).
        r.register("palette", "Command Palette\u{2026}", Some(MenuCategory::View), |c| {
            c.editor.open_palette();
            CommandResult::Handled
        });
        r.register("theme", "Select Theme\u{2026}", Some(MenuCategory::View), |c| {
            c.editor.open_theme_picker();
            CommandResult::Handled
        });
        r.register("menu", "Menu Bar", None, |c| {
            c.editor.palette = None;
            c.editor.prompt = None;
            c.editor.minibuffer = None;
            c.editor.search = None;
            c.editor.diag = None;
            c.editor.outline = None;
            c.editor.theme_picker = None;
            c.editor.pending_keys.clear();
            c.editor.pending_mark = None;
            c.editor.menu = if c.editor.menu.is_some() {
                None
            } else {
                Some(crate::menu::empty())
            };
            CommandResult::Handled
        });

        // Named marks (Task 8 / Effort 5c).
        r.register("set_mark",     "Set Mark\u{2026}",     None, |c| { crate::marks::set_mark(c.editor); CommandResult::Handled });
        r.register("jump_to_mark", "Jump to Mark\u{2026}", None, |c| { crate::marks::jump_to_mark(c.editor); CommandResult::Handled });

        // Jump-back ring (Task 9 / Effort 5c).
        r.register("jump_back",    "Jump Back",    None, |c| { crate::marks::jump_back(c.editor); CommandResult::Handled });
        r.register("jump_forward", "Jump Forward", None, |c| { crate::marks::jump_forward(c.editor); CommandResult::Handled });

        // Format menu — discrete transform commands (Task 1 / Effort 5b).
        r.register("reflow", "Reflow", Some(MenuCategory::Format), |c| {
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Reflow, c.clock, &c.msg_tx);
            CommandResult::Handled
        });
        r.register("unwrap", "Unwrap", Some(MenuCategory::Format), |c| {
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Unwrap, c.clock, &c.msg_tx);
            CommandResult::Handled
        });
        r.register("ventilate", "Ventilate", Some(MenuCategory::Format), |c| {
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Ventilate, c.clock, &c.msg_tx);
            CommandResult::Handled
        });

        // View menu — mouse capture toggle (Task 2 / Effort 5c-m).
        r.register("toggle_mouse_capture", "Toggle Mouse Capture", Some(MenuCategory::View), |c| {
            c.editor.mouse_capture = !c.editor.mouse_capture;
            CommandResult::Handled
        });

        // Diagnostics — quick-fix overlay + navigation + recheck (Task 6–7 / Effort 5f).
        r.register("quick_fix", "Quick Fix\u{2026}", None, |c| {
            let b = c.editor.active();
            if !b.diagnostics.valid_for(b.document.version) {
                c.editor.status = "no diagnostic here".into();
                return CommandResult::Handled;
            }
            let caret = c.editor.active().document.selection.primary().head;
            let diag = c.editor.active().diagnostics.diagnostics.iter()
                .find(|d| d.range.start <= caret && caret <= d.range.end)
                .cloned();
            if let Some(d) = diag {
                c.editor.open_diag(d);
            } else {
                c.editor.status = "no diagnostic here".into();
            }
            CommandResult::Handled
        });
        r.register("diag_next", "Next Diagnostic", None, |c| {
            let b = c.editor.active();
            if !b.diagnostics.valid_for(b.document.version) {
                return CommandResult::Handled;
            }
            let diags = c.editor.active().diagnostics.diagnostics.clone();
            if diags.is_empty() { return CommandResult::Handled; }
            let caret = c.editor.active().document.selection.primary().to();
            let target = diags.iter()
                .find(|d| d.range.start > caret)
                .unwrap_or(&diags[0])
                .range.start;
            unfold_ancestors_of(c.editor, target);
            c.editor.active_mut().document.selection =
                wordcartel_core::selection::Selection::single(target);
            crate::derive::rebuild(c.editor);
            crate::nav::ensure_visible(c.editor);
            CommandResult::Handled
        });
        r.register("diag_prev", "Previous Diagnostic", None, |c| {
            let b = c.editor.active();
            if !b.diagnostics.valid_for(b.document.version) {
                return CommandResult::Handled;
            }
            let diags = c.editor.active().diagnostics.diagnostics.clone();
            if diags.is_empty() { return CommandResult::Handled; }
            let caret = c.editor.active().document.selection.primary().to();
            let last = diags.len() - 1;
            let target = diags.iter()
                .rev()
                .find(|d| d.range.start < caret)
                .unwrap_or(&diags[last])
                .range.start;
            unfold_ancestors_of(c.editor, target);
            c.editor.active_mut().document.selection =
                wordcartel_core::selection::Selection::single(target);
            crate::derive::rebuild(c.editor);
            crate::nav::ensure_visible(c.editor);
            CommandResult::Handled
        });
        r.register("recheck_diagnostics", "Recheck Diagnostics", None, |c| {
            if c.editor.diag_cfg.enabled {
                c.editor.active_mut().diagnostics.arm(c.clock.now_ms(), 0);
            }
            CommandResult::Handled
        });

        // View menu — writing-experience toggles (Task 2 / Effort 5d).
        r.register("toggle_typewriter", "Toggle Typewriter", Some(MenuCategory::View), |c| { c.editor.view_opts.typewriter = !c.editor.view_opts.typewriter; CommandResult::Handled });
        r.register("toggle_focus",      "Toggle Focus Mode", Some(MenuCategory::View), |c| { c.editor.view_opts.focus = !c.editor.view_opts.focus; CommandResult::Handled });
        r.register("toggle_measure",    "Toggle Centered Measure", Some(MenuCategory::View), |c| { c.editor.view_opts.measure = !c.editor.view_opts.measure; crate::derive::rebuild(c.editor); CommandResult::Handled });
        r.register("toggle_wrap_guide", "Toggle Wrap Guide", Some(MenuCategory::View), |c| { c.editor.view_opts.wrap_guide = !c.editor.view_opts.wrap_guide; CommandResult::Handled });
        r.register("toggle_word_count", "Toggle Word Count", Some(MenuCategory::View), |c| { c.editor.view_opts.word_count = !c.editor.view_opts.word_count; CommandResult::Handled });

        // View menu — section folding (Task 10 / Effort 5g).
        r.register("fold_toggle", "Fold/Unfold Section", Some(MenuCategory::View), |c| {
            let caret = c.editor.active().document.selection.primary().head;
            let (blocks, buf) = {
                let b = c.editor.active();
                (b.document.blocks.clone(), b.document.buffer.clone())
            };
            let rope = buf.snapshot();
            let hs = wordcartel_core::outline::headings(&blocks, &rope);
            if let Some(h) = hs.iter().rev().find(|h| h.byte <= caret) {
                let hb = h.byte;
                c.editor.active_mut().folds.toggle(hb);
                let b = c.editor.active();
                let nc = crate::fold::normalize_caret(&b.folds, &b.document.blocks, &b.document.buffer, caret);
                c.editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(nc);
                crate::derive::rebuild(c.editor);
                crate::nav::ensure_visible(c.editor);
            } else {
                c.editor.status = "no heading at cursor".into();
            }
            CommandResult::Handled
        });
        r.register("fold_all", "Fold All Sections", Some(MenuCategory::View), |c| {
            let (blocks, buf) = { let b = c.editor.active(); (b.document.blocks.clone(), b.document.buffer.clone()) };
            c.editor.active_mut().folds.fold_all(&blocks, &buf);
            let caret = c.editor.active().document.selection.primary().head;
            let b = c.editor.active();
            let nc = crate::fold::normalize_caret(&b.folds, &b.document.blocks, &b.document.buffer, caret);
            c.editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(nc);
            crate::derive::rebuild(c.editor);
            crate::nav::ensure_visible(c.editor);
            CommandResult::Handled
        });
        r.register("goto_line", "Go to Line\u{2026}", Some(MenuCategory::View), |c| {
            c.editor.open_minibuffer("Go to line: ", crate::minibuffer::MinibufferKind::GotoLine);
            CommandResult::Handled
        });
        r.register("unfold_all", "Unfold All Sections", Some(MenuCategory::View), |c| {
            c.editor.active_mut().folds.unfold_all();
            crate::derive::rebuild(c.editor);
            crate::nav::ensure_visible(c.editor);
            CommandResult::Handled
        });
        r.register("outline", "Outline\u{2026}", Some(MenuCategory::View), |c| {
            c.editor.open_outline();
            CommandResult::Handled
        });

        // Heading navigation motions (Task 10 / Effort 5g).
        r.register("heading_next",   "Next Heading",   None, |c| { heading_jump(c, Dirn::Next);   CommandResult::Handled });
        r.register("heading_prev",   "Previous Heading", None, |c| { heading_jump(c, Dirn::Prev); CommandResult::Handled });
        r.register("heading_parent", "Parent Heading", None, |c| { heading_jump(c, Dirn::Parent); CommandResult::Handled });

        r
    }

    /// Dispatch by id. Unknown ids surface a status (never a silent no-op, §12.5).
    pub fn dispatch(&self, id: CommandId, ctx: &mut Ctx) -> CommandResult {
        match self.index.get(&id) {
            Some(&i) => (self.entries[i].handler)(ctx),
            None => {
                ctx.editor.status = format!("unknown command: {}", id.0);
                CommandResult::Noop
            }
        }
    }

    /// Resolve a runtime command-id string to the registry's stored `CommandId`
    /// (which wraps a `&'static str`) — without allocating or leaking. Returns
    /// None if no command with that name is registered.
    #[allow(dead_code)] // wired in Task 3
    pub fn resolve_name(&self, name: &str) -> Option<CommandId> {
        self.index.get_key_value(name).map(|(id, _)| *id)
    }

    /// Look up metadata for a registered command.
    #[allow(dead_code)] // wired in Task 3/4
    pub fn meta(&self, id: CommandId) -> Option<&CommandMeta> {
        self.index.get(&id).map(|&i| &self.entries[i].meta)
    }

    /// Iterate registered commands in insertion order, yielding (id, meta) pairs.
    #[allow(dead_code)] // wired in Task 3/4
    pub fn commands(&self) -> impl Iterator<Item = (CommandId, &CommandMeta)> {
        self.entries.iter().map(|e| (e.id, &e.meta))
    }
}

/// Thin adapter: run a built-in `Command` against the Ctx's editor+clock.
fn run(ctx: &mut Ctx, cmd: Command) -> CommandResult {
    commands::run(cmd, ctx.editor, ctx.clock)
}

// ── Fold/heading helpers ──────────────────────────────────────────────────────

enum Dirn { Next, Prev, Parent }

fn heading_jump(c: &mut Ctx, dir: Dirn) {
    let caret = c.editor.active().document.selection.primary().head;
    let (blocks, buf) = { let b = c.editor.active(); (b.document.blocks.clone(), b.document.buffer.clone()) };
    let rope = buf.snapshot();
    let hs = wordcartel_core::outline::headings(&blocks, &rope);
    let target = match dir {
        Dirn::Next => hs.iter().find(|h| h.byte > caret).map(|h| h.byte),
        Dirn::Prev => hs.iter().rev().find(|h| h.byte < caret).map(|h| h.byte),
        Dirn::Parent => {
            let cur = hs.iter().rev().find(|h| h.byte <= caret);
            match cur {
                Some(cur) => hs.iter().rev().find(|h| h.byte < cur.byte && h.level < cur.level).map(|h| h.byte),
                None => None,
            }
        }
    };
    if let Some(t) = target {
        crate::marks::record_jump(c.editor.active_mut(), caret);
        unfold_ancestors_of(c.editor, t);
        c.editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(t);
        crate::derive::rebuild(c.editor);
        crate::nav::ensure_visible(c.editor);
    } else {
        c.editor.status = "no heading".into();
    }
}

/// Unfold every folded heading whose body contains `byte`.
pub(crate) fn unfold_ancestors_of(editor: &mut crate::editor::Editor, byte: usize) {
    let (blocks, buf) = { let b = editor.active(); (b.document.blocks.clone(), b.document.buffer.clone()) };
    let rope = buf.snapshot();
    let anchors: Vec<usize> = editor.active().folds.folded.iter().copied().collect();
    for hb in anchors {
        let body = wordcartel_core::outline::body_range(&blocks, &rope, hb);
        if byte >= body.start && byte < body.end {
            editor.active_mut().folds.folded.remove(&hb);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CaretPlace { UnfoldTo, SnapOut }

pub(crate) fn place_caret_visible(editor: &mut crate::editor::Editor, raw: usize, mode: CaretPlace) -> usize {
    match mode {
        CaretPlace::UnfoldTo => {
            unfold_ancestors_of(editor, raw);
            raw
        }
        CaretPlace::SnapOut => {
            let b = editor.active();
            crate::fold::normalize_caret(&b.folds, &b.document.blocks, &b.document.buffer, raw)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;
    use crate::jobs::InlineExecutor;
    use wordcartel_core::history::Clock;

    struct Z;
    impl Clock for Z { fn now_ms(&self) -> u64 { 0 } }

    #[test]
    fn commands_iterate_in_registration_order_with_meta() {
        let reg = Registry::builtins();
        let ids: Vec<&str> = reg.commands().map(|(id, _)| id.0).collect();
        // deterministic + stable across calls
        let ids2: Vec<&str> = reg.commands().map(|(id, _)| id.0).collect();
        assert_eq!(ids, ids2);
        // every command has a non-empty label
        assert!(reg.commands().all(|(_, m)| !m.label.is_empty()));
        // a known command's meta
        let cut = reg.meta(CommandId("cut")).unwrap();
        assert_eq!(cut.label, "Cut");
        assert_eq!(cut.menu, Some(MenuCategory::Edit));
    }

    #[test]
    fn transforms_are_registered_commands_in_format_category() {
        let reg = Registry::builtins();
        for (id, cat) in [("reflow","Reflow"), ("unwrap","Unwrap"), ("ventilate","Ventilate")] {
            let m = reg.meta(CommandId(id)).unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(m.menu, Some(MenuCategory::Format));
            assert_eq!(m.label, cat);
            assert!(reg.resolve_name(id).is_some());
        }
    }

    #[test]
    fn resolve_name_and_dispatch_still_work_after_refactor() {
        let reg = Registry::builtins();
        assert_eq!(reg.resolve_name("save"), Some(CommandId("save")));
        assert_eq!(reg.resolve_name("nope"), None);
    }

    #[test]
    fn dispatch_save_id_runs_save_handler() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
        let r = reg.dispatch(CommandId("save"), &mut ctx);
        // No path → save handler reports the no-name status (delegates to run()).
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert!(e.status.contains("No file name"));
    }

    #[test]
    fn unknown_command_surfaces_status_not_silent() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
        let r = reg.dispatch(CommandId("nope"), &mut ctx);
        assert_eq!(r, crate::commands::CommandResult::Noop);
        assert!(e.status.contains("unknown command"), "must surface, never silent (§12.5)");
    }

    #[test]
    fn keymap_printable_is_insert_fallthrough_and_arrows_are_ids() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let a = KeyEvent { code: KeyCode::Char('a'), modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE };
        assert!(matches!(crate::input::key_to_command_id(a), Some(crate::input::KeyAction::Insert('a'))));
        let shift_left = KeyEvent { code: KeyCode::Left, modifiers: KeyModifiers::SHIFT,
            kind: KeyEventKind::Press, state: KeyEventState::NONE };
        assert!(matches!(crate::input::key_to_command_id(shift_left),
            Some(crate::input::KeyAction::Id(CommandId("select_left")))));
    }

    #[test]
    fn keymap_ctrl_e_is_filter() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let ctrl_e = KeyEvent {
            code: KeyCode::Char('e'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(
            crate::input::key_to_command_id(ctrl_e),
            Some(crate::input::KeyAction::Id(CommandId("filter")))
        ));
    }

    #[test]
    fn builtin_command_ids_are_unique() {
        let reg = Registry::builtins();
        let mut seen = std::collections::HashSet::new();
        for (id, _) in reg.commands() {
            assert!(seen.insert(id.0), "duplicate command id: {}", id.0);
        }
    }

    #[test]
    fn keymap_ctrl_t_is_transform() {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let ctrl_t = KeyEvent {
            code: KeyCode::Char('t'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert!(matches!(
            crate::input::key_to_command_id(ctrl_t),
            Some(crate::input::KeyAction::Id(CommandId("transform")))
        ));
    }

    #[test]
    fn resolve_name_recovers_static_command_id() {
        let reg = Registry::builtins();
        assert_eq!(reg.resolve_name("cut"), Some(CommandId("cut")));
        assert_eq!(reg.resolve_name("save"), Some(CommandId("save")));
        assert_eq!(reg.resolve_name("definitely-not-a-command"), None);
    }

    // -----------------------------------------------------------------------
    // Task 12 (Effort 5g): diag_next into a folded section auto-unfolds
    // -----------------------------------------------------------------------

    #[test]
    fn diag_next_into_fold_auto_unfolds() {
        // Build a buffer with a folded ## A section and a diagnostic inside it.
        // Seed the DiagStore directly (no real Harper worker).
        let doc = "# Top\nintro\n## A\nbad_word here\nmore\n## B\n";
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        let a_byte = doc.find("## A").unwrap();
        let bad_byte = doc.find("bad_word").unwrap();

        // Seed the DiagStore with a diagnostic inside ## A's body.
        let v = ed.active().document.version;
        ed.active_mut().diagnostics.diagnostics = vec![
            wordcartel_core::diagnostics::Diagnostic {
                range: bad_byte..(bad_byte + "bad_word".len()),
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                message: "x".into(),
                suggestions: vec![],
            }
        ];
        ed.active_mut().diagnostics.computed_version = v;

        // Fold ## A AFTER seeding diagnostics (version unchanged).
        ed.active_mut().folds.toggle(a_byte);
        crate::derive::rebuild(&mut ed);

        // Place caret before the diagnostic (at start of doc).
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);

        // Dispatch diag_next.
        dispatch_id(&mut ed, "diag_next");

        // Caret must be at the diagnostic's start.
        assert_eq!(ed.active().document.selection.primary().head, bad_byte,
            "diag_next must jump caret to the diagnostic");
        // ## A fold must be cleared.
        assert!(!ed.active().folds.folded.contains(&a_byte),
            "## A fold must be cleared when diag_next lands inside its body");
    }

    #[test]
    fn diag_prev_into_fold_auto_unfolds() {
        // Build a buffer with a folded ## A section and a diagnostic inside it.
        // Seed the DiagStore directly (no real Harper worker).
        let doc = "# Top\nintro\n## A\nbad_word here\nmore\n## B\n";
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        let a_byte = doc.find("## A").unwrap();
        let bad_byte = doc.find("bad_word").unwrap();

        // Seed the DiagStore with a diagnostic inside ## A's body.
        let v = ed.active().document.version;
        ed.active_mut().diagnostics.diagnostics = vec![
            wordcartel_core::diagnostics::Diagnostic {
                range: bad_byte..(bad_byte + "bad_word".len()),
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                message: "x".into(),
                suggestions: vec![],
            }
        ];
        ed.active_mut().diagnostics.computed_version = v;

        // Fold ## A AFTER seeding diagnostics (version unchanged).
        ed.active_mut().folds.toggle(a_byte);
        crate::derive::rebuild(&mut ed);

        // Place caret after the diagnostic.
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(doc.find("## B").unwrap());

        // Dispatch diag_prev.
        dispatch_id(&mut ed, "diag_prev");

        // Caret must be at the diagnostic's start.
        assert_eq!(ed.active().document.selection.primary().head, bad_byte,
            "diag_prev must jump caret to the diagnostic");
        // ## A fold must be cleared.
        assert!(!ed.active().folds.folded.contains(&a_byte),
            "## A fold must be cleared when diag_prev lands inside its body");
    }

    // Helper: build a Ctx and dispatch a command id against the given Editor.
    fn dispatch_id(ed: &mut Editor, id: &'static str) {
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: ed, clock: &clk, executor: &ex, msg_tx: tx };
        reg.dispatch(CommandId(id), &mut ctx);
    }

    #[test]
    fn fold_toggle_folds_caret_section_and_moves_caret_to_heading() {
        let doc = "# Top\nintro\n## A\nbody1\nbody2\n## B\n";
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        crate::derive::rebuild(&mut ed);
        // caret inside ## A's body
        let inside = doc.find("body2").unwrap();
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(inside);
        dispatch_id(&mut ed, "fold_toggle");
        let a = doc.find("## A").unwrap();
        assert!(ed.active().folds.folded.contains(&a));
        // caret moved out of the now-hidden body, onto the heading
        assert_eq!(ed.active().document.selection.primary().head, a);
    }

    #[test]
    fn heading_next_prev_parent_navigate_and_push_ring() {
        let doc = "# Top\nintro\n## A\nbody\n### A1\nx\n## B\n";
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        crate::derive::rebuild(&mut ed);
        let top = doc.find("# Top").unwrap();
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(top);
        dispatch_id(&mut ed, "heading_next");
        assert_eq!(ed.active().document.selection.primary().head, doc.find("## A").unwrap());
        // ring got the origin pushed
        assert!(ed.active().jump_ring.contains(&top));
        // parent of ### A1 is ## A
        let a1 = doc.find("### A1").unwrap();
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(a1);
        dispatch_id(&mut ed, "heading_parent");
        assert_eq!(ed.active().document.selection.primary().head, doc.find("## A").unwrap());
    }

    #[test]
    fn heading_prev_navigates_and_pushes_ring() {
        let doc = "# Top\nintro\n## A\nbody\n### A1\nx\n## B\n";
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        crate::derive::rebuild(&mut ed);
        let b = doc.find("## B").unwrap();
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(b);
        dispatch_id(&mut ed, "heading_prev");
        assert_eq!(ed.active().document.selection.primary().head, doc.find("### A1").unwrap());
        assert!(ed.active().jump_ring.contains(&b));
    }
}
