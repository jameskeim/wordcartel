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
pub enum MenuCategory { File, Edit, Format, View, Settings, Export }

pub const MENU_ORDER: [MenuCategory; 6] =
    [MenuCategory::File, MenuCategory::Edit, MenuCategory::Format, MenuCategory::View, MenuCategory::Settings, MenuCategory::Export];

/// The live-state mark a stateful menu command interpolates into its row label.
/// Exhaustive — adding a variant here is intentional and must be handled in every match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuMark { OnOff(bool), Value(&'static str) }

#[derive(Clone, Copy)]
pub struct CommandMeta {
    pub label: &'static str,
    pub menu: Option<MenuCategory>,
    /// Optional live-state provider — evaluated at menu-build time against `&Editor`.
    /// `None` for stateless commands (their static label renders unchanged).
    pub state: Option<fn(&crate::editor::Editor) -> MenuMark>,
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
        self.entries.push(CommandEntry { id: cid, handler, meta: CommandMeta { label, menu, state: None } });
    }

    fn register_stateful(&mut self, id: &'static str, label: &'static str, menu: Option<MenuCategory>,
                         state: fn(&crate::editor::Editor) -> MenuMark, handler: Handler) {
        let cid = CommandId(id);
        self.index.insert(cid, self.entries.len());
        self.entries.push(CommandEntry { id: cid, handler, meta: CommandMeta { label, menu, state: Some(state) } });
    }

    #[allow(clippy::too_many_lines)] // the command registry data table — one entry per command
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
        r.register("move_screen_top",    "Move to Screen Top",    None, |c| run(c, Command::Move { dir: Dir::ScreenTop,    extend: false }));
        r.register("move_screen_bottom", "Move to Screen Bottom", None, |c| run(c, Command::Move { dir: Dir::ScreenBottom, extend: false }));
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
            c.editor.open_minibuffer("sh> ", crate::minibuffer::MinibufferKind::Filter);
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
        r.register("new", "New", Some(MenuCategory::File), |c| {
            crate::prompts::request_new(c.editor, c.executor, c.clock, &c.msg_tx);
            CommandResult::Handled
        });
        r.register("open", "Open…", Some(MenuCategory::File), |c| {
            let dir = c.editor.active().document.path.as_ref()
                .and_then(|p| p.parent())
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            c.editor.open_file_browser(dir);
            CommandResult::Handled
        });
        r.register("save", "Save", Some(MenuCategory::File), crate::save::dispatch_save);
        r.register("save_as", "Save As…", Some(MenuCategory::File), |c| {
            crate::prompts::open_save_as(c.editor);
            CommandResult::Handled
        });
        r.register("save_and_quit", "Save and Quit", Some(MenuCategory::File), |c| {
            crate::save::dispatch_save_and_quit(c);
            CommandResult::Handled
        });
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
        r.register("export_tex", "Export LaTeX", Some(MenuCategory::Export), |c| {
            crate::export::run_export(c.editor, "tex", &c.msg_tx);
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
            c.editor.file_browser = None;
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

        // Numbered bookmarks ^K0-9/^Q0-9 (Task 4 / Effort 9b).
        // Handler is a fn pointer — runtime loop can't capture `ch`, so use a macro with literal digits.
        macro_rules! register_bookmarks {
            ($r:expr, $($d:literal => $ch:literal),+ $(,)?) => {$(
                $r.register(concat!("set_bookmark_", $d), concat!("Set Bookmark ", $d), None,
                    |c| { crate::marks::set_char_mark(c.editor, $ch);
                          c.editor.status = concat!("bookmark ", $d, " set").to_string();
                          CommandResult::Handled });
                $r.register(concat!("jump_bookmark_", $d), concat!("Jump to Bookmark ", $d), None,
                    |c| { if crate::marks::jump_char_mark(c.editor, $ch) {
                              c.editor.status = concat!("jumped to bookmark ", $d).to_string();
                          } else {
                              c.editor.status = concat!("no bookmark ", $d).to_string();
                          }
                          CommandResult::Handled });
            )+};
        }
        register_bookmarks!(r,
            "0" => '0', "1" => '1', "2" => '2', "3" => '3', "4" => '4',
            "5" => '5', "6" => '6', "7" => '7', "8" => '8', "9" => '9');

        // Jump-back ring (Task 9 / Effort 5c).
        r.register("jump_back",    "Jump Back",    None, |c| { crate::marks::jump_back(c.editor); CommandResult::Handled });
        r.register("jump_forward", "Jump Forward", None, |c| { crate::marks::jump_forward(c.editor); CommandResult::Handled });

        // Marked block creation (Task 2 / Effort 9A).
        r.register("block_begin",               "Set Block Begin",         Some(MenuCategory::Edit), |c| { crate::blocks_marked::block_begin(c.editor); CommandResult::Handled });
        r.register("block_end",                 "Set Block End",           Some(MenuCategory::Edit), |c| { crate::blocks_marked::block_end(c.editor); CommandResult::Handled });
        r.register("mark_block_from_selection", "Mark Block from Selection", Some(MenuCategory::Edit), |c| { crate::blocks_marked::mark_block_from_selection(c.editor); CommandResult::Handled });
        // Block → selection bridge (A11.3, Task 1.1 / command-surface curation). `menu: None`
        // for now — flips to `Some(MenuCategory::Block)` once that category lands (Task 2.1).
        r.register("select_marked_block", "Select Block", None,
            |c| { crate::blocks_marked::select_marked_block(c.editor); CommandResult::Handled });

        // Marked block operations (Task 3 / Effort 9A).
        r.register("block_copy",          "Copy Block",        Some(MenuCategory::Edit), |c| { crate::blocks_marked::block_copy(c.editor, c.clock);   CommandResult::Handled });
        r.register("block_move",          "Move Block",        Some(MenuCategory::Edit), |c| { crate::blocks_marked::block_move(c.editor, c.clock);   CommandResult::Handled });
        r.register("block_delete",        "Delete Block",      Some(MenuCategory::Edit), |c| { crate::blocks_marked::block_delete(c.editor, c.clock); CommandResult::Handled });
        r.register("block_jump_begin",    "Jump to Block Begin", Some(MenuCategory::Edit), |c| { crate::blocks_marked::block_jump_begin(c.editor);    CommandResult::Handled });
        r.register("block_jump_end",      "Jump to Block End",   Some(MenuCategory::Edit), |c| { crate::blocks_marked::block_jump_end(c.editor);      CommandResult::Handled });
        r.register("block_toggle_hidden", "Toggle Block Hidden", Some(MenuCategory::Edit), |c| { crate::blocks_marked::block_toggle_hidden(c.editor); CommandResult::Handled });
        r.register("block_clear",         "Clear Block",         Some(MenuCategory::Edit), |c| { crate::blocks_marked::block_clear(c.editor);         CommandResult::Handled });
        // Marked block write-to-file (Task 4 / Effort 9A).
        r.register("block_write", "Write Block to File\u{2026}", Some(MenuCategory::File), |c| { crate::blocks_marked::block_write(c.editor); CommandResult::Handled });

        // Effort 6: send-to-scratch verbs.
        r.register("copy_block_to_scratch", "Copy Block to Scratch", Some(MenuCategory::Edit), |c| { crate::scratch::copy_block_to_scratch(c.editor, c.clock); CommandResult::Handled });
        r.register("move_block_to_scratch", "Move Block to Scratch", Some(MenuCategory::Edit), |c| { crate::scratch::move_block_to_scratch(c.editor, c.clock); CommandResult::Handled });

        // Effort 6: workspace navigation.
        r.register("next_buffer", "Next Buffer", Some(MenuCategory::View), |c| { crate::workspace::next_buffer(c.editor); CommandResult::Handled });
        r.register("prev_buffer", "Previous Buffer", Some(MenuCategory::View), |c| { crate::workspace::prev_buffer(c.editor); CommandResult::Handled });
        r.register("goto_scratch", "Go to Scratch Buffer", Some(MenuCategory::View), |c| { crate::workspace::goto_scratch(c.editor); CommandResult::Handled });
        r.register("switch_buffer", "Switch Buffer\u{2026}", Some(MenuCategory::View), |c| { c.editor.open_buffer_switcher(); CommandResult::Handled });
        r.register("close_buffer", "Close Buffer", Some(MenuCategory::File), |c| { crate::workspace::close_buffer(c.editor); CommandResult::Handled });

        // Format menu — discrete transform commands (Task 1 / Effort 5b).
        r.register("reflow", "Reflow", Some(MenuCategory::Format), |c| {
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Reflow, None, c.clock, &c.msg_tx);
            CommandResult::Handled
        });
        r.register("unwrap", "Unwrap", Some(MenuCategory::Format), |c| {
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Unwrap, None, c.clock, &c.msg_tx);
            CommandResult::Handled
        });
        r.register("ventilate", "Ventilate", Some(MenuCategory::Format), |c| {
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Ventilate, None, c.clock, &c.msg_tx);
            CommandResult::Handled
        });
        r.register("reflow_buffer", "Reflow Buffer", Some(MenuCategory::Format), |c| {
            let len = c.editor.active().document.buffer.len();
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Reflow, Some(0..len), c.clock, &c.msg_tx);
            CommandResult::Handled
        });
        r.register("unwrap_buffer", "Unwrap Buffer", Some(MenuCategory::Format), |c| {
            let len = c.editor.active().document.buffer.len();
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Unwrap, Some(0..len), c.clock, &c.msg_tx);
            CommandResult::Handled
        });
        r.register("ventilate_buffer", "Ventilate Buffer", Some(MenuCategory::Format), |c| {
            let len = c.editor.active().document.buffer.len();
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Ventilate, Some(0..len), c.clock, &c.msg_tx);
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
        r.register_stateful("toggle_measure", "Toggle Centered Measure", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.measure),
            |c| { c.editor.view_opts.measure = !c.editor.view_opts.measure; crate::derive::rebuild(c.editor); CommandResult::Handled });
        r.register_stateful("toggle_wrap_guide", "Toggle Wrap Guide", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.wrap_guide),
            |c| { c.editor.view_opts.wrap_guide = !c.editor.view_opts.wrap_guide; CommandResult::Handled });
        r.register_stateful("toggle_word_count", "Toggle Word Count", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.word_count),
            |c| { c.editor.view_opts.word_count = !c.editor.view_opts.word_count; CommandResult::Handled });

        // View menu — section folding (Task 10 / Effort 5g).
        r.register("fold_toggle", "Fold/Unfold Section", Some(MenuCategory::View), |c| {
            let caret = c.editor.active().document.selection.primary().head;
            let (blocks, buf) = {
                let b = c.editor.active();
                (b.document.blocks().clone(), b.document.buffer.clone())
            };
            let rope = buf.snapshot();
            let hs = wordcartel_core::outline::headings(&blocks, &rope);
            if let Some(h) = hs.iter().rev().find(|h| h.byte <= caret) {
                let hb = h.byte;
                c.editor.active_mut().folds.toggle(hb);
                let b = c.editor.active();
                let nc = crate::fold::normalize_caret(&b.folds, b.document.blocks(), &b.document.buffer, caret);
                c.editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(nc);
                crate::derive::rebuild(c.editor);
                crate::nav::ensure_visible(c.editor);
            } else {
                c.editor.status = "no heading at cursor".into();
            }
            CommandResult::Handled
        });
        r.register("fold_all", "Fold All Sections", Some(MenuCategory::View), |c| {
            let (blocks, buf) = { let b = c.editor.active(); (b.document.blocks().clone(), b.document.buffer.clone()) };
            c.editor.active_mut().folds.fold_all(&blocks, &buf);
            let caret = c.editor.active().document.selection.primary().head;
            let b = c.editor.active();
            let nc = crate::fold::normalize_caret(&b.folds, b.document.blocks(), &b.document.buffer, caret);
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
        r.register_stateful("menu_bar_pin", "Pin Menu Bar", Some(MenuCategory::View),
            |e| MenuMark::Value(match e.menu_bar_mode {
                crate::config::MenuBarMode::Pinned => "Pinned",
                crate::config::MenuBarMode::Auto   => "Auto",
                crate::config::MenuBarMode::Hidden => "Hidden",
            }),
            |c| {
                use crate::config::MenuBarMode;
                let target = if c.editor.menu_bar_mode == MenuBarMode::Pinned {
                    c.editor.menu_bar_unpinned_mode
                } else { MenuBarMode::Pinned };
                c.editor.set_menu_bar_mode(target);
                CommandResult::Handled
            });

        // Chrome density — scrollbar, status-line, menu-bar set-per-state + representatives (A3).
        // Scrollbar: set-per-state (palette-only) + 3-state cycle representative (View, state-in-label).
        use crate::config::TransientMode;
        r.register("scrollbar_off",  "Scrollbar: Off",  None, |c| { c.editor.set_scrollbar_mode(TransientMode::Off);  CommandResult::Handled });
        r.register("scrollbar_auto", "Scrollbar: Auto", None, |c| { c.editor.set_scrollbar_mode(TransientMode::Auto); CommandResult::Handled });
        r.register("scrollbar_on",   "Scrollbar: On",   None, |c| { c.editor.set_scrollbar_mode(TransientMode::On);   CommandResult::Handled });
        r.register_stateful("cycle_scrollbar", "Scrollbar", Some(MenuCategory::View),
            |e| MenuMark::Value(match e.scrollbar_mode {
                TransientMode::Off => "Off", TransientMode::Auto => "Auto", TransientMode::On => "On" }),
            |c| { let next = match c.editor.scrollbar_mode {
                      TransientMode::Off => TransientMode::Auto, TransientMode::Auto => TransientMode::On,
                      TransientMode::On  => TransientMode::Off };
                  c.editor.set_scrollbar_mode(next); CommandResult::Handled });

        // Status line: set-per-state (palette-only) + 2-state toggle representative (View, state-in-label).
        r.register("status_line_auto", "Status Line: Auto", None, |c| { c.editor.set_status_line_mode(TransientMode::Auto); CommandResult::Handled });
        r.register("status_line_on",   "Status Line: On",   None, |c| { c.editor.set_status_line_mode(TransientMode::On);   CommandResult::Handled });
        r.register_stateful("toggle_status_line", "Status Line", Some(MenuCategory::View),
            |e| MenuMark::Value(match e.status_line_mode { TransientMode::On => "On", TransientMode::Auto | TransientMode::Off => "Auto" }),
            |c| { let next = if c.editor.status_line_mode == TransientMode::On { TransientMode::Auto } else { TransientMode::On };
                  c.editor.set_status_line_mode(next); CommandResult::Handled });

        // Startup splash: set-per-state (palette-only) + 2-state toggle representative
        // (View, OnOff mark). All three route through Editor::set_splash (contract law 6);
        // the splash paints only at launch, so a change takes effect on the NEXT run.
        r.register("splash_on",  "Splash: On",  None, |c| { c.editor.set_splash(true);
            c.editor.status = "splash: on (takes effect next launch)".into(); CommandResult::Handled });
        r.register("splash_off", "Splash: Off", None, |c| { c.editor.set_splash(false);
            c.editor.status = "splash: off (takes effect next launch)".into(); CommandResult::Handled });
        r.register_stateful("toggle_splash", "Startup Splash", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.splash),
            |c| { let next = !c.editor.view_opts.splash; c.editor.set_splash(next);
                  c.editor.status = if next { "splash: on (takes effect next launch)".into() }
                                    else { "splash: off (takes effect next launch)".into() };
                  CommandResult::Handled });

        // Menu bar: deterministic set-per-state (palette-only). menu_bar_pin remains the View representative.
        use crate::config::MenuBarMode;
        r.register("menu_bar_hidden", "Menu Bar: Hidden", None, |c| { c.editor.set_menu_bar_mode(MenuBarMode::Hidden); CommandResult::Handled });
        r.register("menu_bar_auto",   "Menu Bar: Auto",   None, |c| { c.editor.set_menu_bar_mode(MenuBarMode::Auto);   CommandResult::Handled });
        r.register("menu_bar_pinned", "Menu Bar: Pinned", None, |c| { c.editor.set_menu_bar_mode(MenuBarMode::Pinned); CommandResult::Handled });

        // Clipboard provider: set-per-state (palette-only) + 4-state cycle representative
        // (Settings, state-in-label). C3 command-surface conformance.
        use crate::config::ClipboardProvider;
        r.register("clipboard_provider_auto",   "Clipboard: Auto",   None, |c| { c.editor.set_clipboard_provider(ClipboardProvider::Auto);   CommandResult::Handled });
        r.register("clipboard_provider_native", "Clipboard: Native", None, |c| { c.editor.set_clipboard_provider(ClipboardProvider::Native); CommandResult::Handled });
        r.register("clipboard_provider_osc52",  "Clipboard: OSC 52", None, |c| { c.editor.set_clipboard_provider(ClipboardProvider::Osc52);  CommandResult::Handled });
        r.register("clipboard_provider_off",    "Clipboard: Off",    None, |c| { c.editor.set_clipboard_provider(ClipboardProvider::Off);    CommandResult::Handled });
        r.register_stateful("clipboard_provider_cycle", "Clipboard", Some(MenuCategory::Settings),
            |e| MenuMark::Value(match e.clipboard_provider {
                ClipboardProvider::Auto => "Auto", ClipboardProvider::Native => "Native",
                ClipboardProvider::Osc52 => "OSC 52", ClipboardProvider::Off => "Off" }),
            |c| { let next = match c.editor.clipboard_provider {
                      ClipboardProvider::Auto => ClipboardProvider::Native,
                      ClipboardProvider::Native => ClipboardProvider::Osc52,
                      ClipboardProvider::Osc52 => ClipboardProvider::Off,
                      ClipboardProvider::Off => ClipboardProvider::Auto };
                  c.editor.set_clipboard_provider(next); CommandResult::Handled });

        // Heading navigation motions (Task 10 / Effort 5g).
        r.register("heading_next",   "Next Heading",   None, |c| { heading_jump(c, Dirn::Next);   CommandResult::Handled });
        r.register("heading_prev",   "Previous Heading", None, |c| { heading_jump(c, Dirn::Prev); CommandResult::Handled });
        r.register("heading_parent", "Parent Heading", None, |c| { heading_jump(c, Dirn::Parent); CommandResult::Handled });

        // WordStar viewport scroll (Task 6 / Effort 9b): ^W/^Z scroll one row, caret clamped.
        r.register("scroll_line_up",   "Scroll Line Up",   None, |c| { crate::nav::scroll_line_up(c.editor);   CommandResult::Handled });
        r.register("scroll_line_down", "Scroll Line Down", None, |c| { crate::nav::scroll_line_down(c.editor); CommandResult::Handled });

        // Settings menu — runtime keymap preset switching (D1+A5).
        // keymap_cua/keymap_wordstar are palette-only (menu: None); the menu shows one
        // cycle row (keymap_next) whose label reflects the active preset.
        r.register("keymap_cua", "Keymap: CUA", None, |c| {
            switch_keymap_preset(c.editor, "cua");
            CommandResult::Handled
        });
        r.register("keymap_wordstar", "Keymap: WordStar", None, |c| {
            switch_keymap_preset(c.editor, "wordstar");
            CommandResult::Handled
        });
        r.register_stateful("keymap_next", "Keymap", Some(MenuCategory::Settings),
            |e| MenuMark::Value(if e.active_keymap_preset == "wordstar" { "WordStar" } else { "CUA" }),
            |c| {
                let next = if c.editor.active_keymap_preset == "cua" { "wordstar" } else { "cua" };
                switch_keymap_preset(c.editor, next);
                CommandResult::Handled
            });
        r.register("set_wrap_column", "Set Wrap Column\u{2026}", Some(MenuCategory::Settings), |c| {
            c.editor.open_minibuffer("Wrap column: ", crate::minibuffer::MinibufferKind::WrapColumn);
            CommandResult::Handled
        });
        // toggle_canvas and toggle_chrome MUST be registered BEFORE save_settings
        // (journey_palette_end relies on save_settings being the last command dispatched
        // from End+Enter — spec D3 / A.7).
        r.register_stateful("toggle_canvas", "Canvas: Opaque/Transparent", Some(MenuCategory::Settings),
            |e| MenuMark::Value(match e.canvas {
                wordcartel_core::theme::CanvasMode::Opaque       => "Opaque",
                wordcartel_core::theme::CanvasMode::Transparent  => "Transparent",
            }),
            |c| { toggle_canvas(c.editor); CommandResult::Handled });
        r.register_stateful("toggle_chrome", "Chrome: Full/Zen", Some(MenuCategory::Settings),
            |e| MenuMark::Value(match e.chrome_disposition {
                wordcartel_core::theme::ChromeDisposition::Full => "Full",
                wordcartel_core::theme::ChromeDisposition::Zen  => "Zen",
            }),
            |c| { toggle_chrome(c.editor); CommandResult::Handled });
        r.register("save_settings", "Save Settings", Some(MenuCategory::Settings), |c| {
            c.editor.settings_save_requested = true;
            CommandResult::Handled
        });

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
    pub fn resolve_name(&self, name: &str) -> Option<CommandId> {
        self.index.get_key_value(name).map(|(id, _)| *id)
    }

    /// Look up metadata for a registered command.
    pub fn meta(&self, id: CommandId) -> Option<&CommandMeta> {
        self.index.get(&id).map(|&i| &self.entries[i].meta)
    }

    /// Iterate registered commands in insertion order, yielding (id, meta) pairs.
    pub fn commands(&self) -> impl Iterator<Item = (CommandId, &CommandMeta)> {
        self.entries.iter().map(|e| (e.id, &e.meta))
    }
}

/// Request a keymap preset switch: no-op with a status when already active; else set the
/// preset and the rebuild flag — the run loop swaps the trie between reduces (spec D2).
fn switch_keymap_preset(editor: &mut crate::editor::Editor, preset: &str) {
    if editor.active_keymap_preset == preset {
        editor.status = format!("keymap: {preset} (already active)");
        return;
    }
    editor.active_keymap_preset = preset.to_string();
    editor.keymap_rebuild = true;
    editor.status = format!("keymap: {preset}");
}

/// Toggle the chrome disposition (Full ⇄ Zen). Mirrors `switch_keymap_preset` in structure:
/// sets the flag and gives an honest status. Four arms (spec D3 / grounding A.7):
///   • monochrome/cue theme: NO flip (derivation always skips on monochrome).
///   • non-Rgb bases (terminal-plain, terminal-ansi): flips + persists, warns "no effect".
///   • Rgb bases at Ansi16 depth: flips + persists, warns "no effect at 16-color depth".
///   • normal: flips + "chrome: full"/"chrome: zen".
fn toggle_chrome(editor: &mut crate::editor::Editor) {
    use wordcartel_core::theme::{ChromeDisposition, Color, Depth};
    // Arm 1 — cue/monochrome theme: disposition flip has no visible effect (derive_chrome
    // is a no-op on monochrome themes); inform the user without changing state.
    if editor.theme.monochrome {
        editor.status = "chrome: n/a (cue mode)".into();
        return;
    }
    // Flip the disposition and request a full re-derive in the between-reduces arm.
    let new_disp = match editor.chrome_disposition {
        ChromeDisposition::Full => ChromeDisposition::Zen,
        ChromeDisposition::Zen  => ChromeDisposition::Full,
    };
    // Apply the whole density bundle for the new disposition (color + visibility),
    // then request the re-derive. Re-selecting a preset re-applies its bundle over
    // unsaved runtime state (spec §1.5 — runtime-clobber). rebuild for measure.
    crate::density::apply_bundle(editor, crate::density::bundle_for(new_disp));
    editor.theme_rederive = true;
    crate::derive::rebuild(editor); // measure change affects layout
    let label = match new_disp { ChromeDisposition::Full => "full", ChromeDisposition::Zen => "zen" };
    // Arm 2 — non-Rgb bases: derive_chrome early-returns on non-Rgb (Color::Default /
    // named colors). The flip is recorded and persists, but the visible chrome is unchanged.
    let rgb_bases = matches!(editor.theme.base_bg, Color::Rgb { .. })
        && matches!(editor.theme.base_fg, Color::Rgb { .. });
    if !rgb_bases {
        let name = editor.theme.name.clone();
        editor.status = format!("chrome: {label} (no effect: {name} has fixed chrome)");
        return;
    }
    // Arm 3 — Rgb theme at Ansi16 depth: the fixed 5-face Ansi16 policy applied by
    // resolve_theme overrides the derived faces; toggling disposition has no visible effect.
    if editor.depth == Depth::Ansi16 {
        editor.status = format!("chrome: {label} (no effect at 16-color depth)");
        return;
    }
    // Normal arm: derived Rgb theme at Truecolor/256; the rederive will visibly change chrome.
    editor.status = format!("chrome: {label}");
}

/// Flip the canvas opacity. Render-only — no re-derive. The flip always persists (canvas is a
/// user preference that outlives the current theme); the status is honest about visibility:
///   • Rgb theme at a color depth: "canvas: opaque"/"canvas: transparent".
///   • non-Rgb base_bg, or Depth::None: flips + persists, "no effect: {name} has no canvas".
fn toggle_canvas(editor: &mut crate::editor::Editor) {
    use wordcartel_core::theme::{CanvasMode, Color, Depth};
    let new_mode = match editor.canvas {
        CanvasMode::Opaque      => CanvasMode::Transparent,
        CanvasMode::Transparent => CanvasMode::Opaque,
    };
    editor.canvas = new_mode;
    let label = match new_mode { CanvasMode::Opaque => "opaque", CanvasMode::Transparent => "transparent" };
    // No canvas to paint: non-Rgb base_bg (terminal-* themes) or the None (cue) depth.
    let has_canvas = matches!(editor.theme.base_bg, Color::Rgb { .. }) && editor.depth != Depth::None;
    if !has_canvas {
        let name = editor.theme.name.clone();
        editor.status = format!("canvas: {label} (no effect: {name} has no canvas)");
        return;
    }
    editor.status = format!("canvas: {label}");
}

/// Thin adapter: run a built-in `Command` against the Ctx's editor+clock.
fn run(ctx: &mut Ctx, cmd: Command) -> CommandResult {
    commands::run(cmd, ctx.editor, ctx.clock)
}

// ── Fold/heading helpers ──────────────────────────────────────────────────────

enum Dirn { Next, Prev, Parent }

fn heading_jump(c: &mut Ctx, dir: Dirn) {
    let caret = c.editor.active().document.selection.primary().head;
    let (blocks, buf) = { let b = c.editor.active(); (b.document.blocks().clone(), b.document.buffer.clone()) };
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
    let (blocks, buf) = { let b = editor.active(); (b.document.blocks().clone(), b.document.buffer.clone()) };
    let rope = buf.snapshot();
    let anchors: Vec<usize> = editor.active().folds.folded().iter().copied().collect();
    for hb in anchors {
        let body = wordcartel_core::outline::body_range(&blocks, &rope, hb);
        if byte >= body.start && byte < body.end {
            editor.active_mut().folds.remove(hb);
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
            crate::fold::normalize_caret(&b.folds, b.document.blocks(), &b.document.buffer, raw)
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
        for (id, label) in [
            ("reflow",           "Reflow"),
            ("unwrap",           "Unwrap"),
            ("ventilate",        "Ventilate"),
            ("reflow_buffer",    "Reflow Buffer"),
            ("unwrap_buffer",    "Unwrap Buffer"),
            ("ventilate_buffer", "Ventilate Buffer"),
        ] {
            let m = reg.meta(CommandId(id)).unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(m.menu, Some(MenuCategory::Format));
            assert_eq!(m.label, label);
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
    fn clipboard_provider_commands_registered_with_correct_menu_tags() {
        // Real accessors: resolve_name(&str) -> Option<CommandId> (registry.rs:571);
        // meta(CommandId) -> Option<&CommandMeta> (registry.rs:577). CommandEntry is private.
        let reg = Registry::builtins();
        let meta = |id: &str| reg.meta(reg.resolve_name(id).expect(id)).expect(id);
        for id in ["clipboard_provider_auto", "clipboard_provider_native",
                   "clipboard_provider_osc52", "clipboard_provider_off"] {
            assert_eq!(meta(id).menu, None, "{id} is palette-only");
        }
        let cyc = meta("clipboard_provider_cycle");
        assert_eq!(cyc.menu, Some(MenuCategory::Settings), "cycle is the Settings menu representative");
        assert!(cyc.state.is_some(), "cycle carries state-in-label");
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
        // No path → save handler opens the Save-As minibuffer (Effort 7, Task 3).
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert!(matches!(e.minibuffer.as_ref().map(|m| m.kind),
            Some(crate::minibuffer::MinibufferKind::SaveAs)),
            "unnamed save opens the Save-As minibuffer");
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
    fn switch_buffer_is_registered_in_view_menu() {
        let reg = Registry::builtins();
        let m = reg.meta(CommandId("switch_buffer"))
            .expect("switch_buffer must be registered");
        assert_eq!(m.label, "Switch Buffer\u{2026}");
        assert_eq!(m.menu, Some(MenuCategory::View));
    }

    #[test]
    fn builtin_command_ids_are_unique() {
        let reg = Registry::builtins();
        let mut seen = std::collections::HashSet::new();
        for (id, _) in reg.commands() {
            assert!(seen.insert(id.0), "duplicate command id: {}", id.0);
        }
    }

    // -----------------------------------------------------------------------
    // Task 2 (D1+A5): Settings menu keymap commands
    // -----------------------------------------------------------------------

    #[test]
    fn settings_commands_registered_in_settings_category() {
        let reg = Registry::builtins();
        // keymap_cua/keymap_wordstar are palette-only now; keymap_next is the single cycle row.
        for (id, label) in [
            ("keymap_next",      "Keymap"),
            ("set_wrap_column",  "Set Wrap Column\u{2026}"),
            ("toggle_chrome",    "Chrome: Full/Zen"),
            ("save_settings",    "Save Settings"),
        ] {
            let m = reg.meta(CommandId(id)).unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(m.label, label, "label mismatch for {id}");
            assert_eq!(m.menu, Some(MenuCategory::Settings), "menu category mismatch for {id}");
        }
        // Confirm demotion.
        assert_eq!(reg.meta(CommandId("keymap_cua")).unwrap().menu, None, "keymap_cua must be palette-only");
        assert_eq!(reg.meta(CommandId("keymap_wordstar")).unwrap().menu, None, "keymap_wordstar must be palette-only");
    }

    // -----------------------------------------------------------------------
    // Task 2 (canvas-transparency): toggle_canvas — honest arms
    // -----------------------------------------------------------------------

    #[test]
    fn toggle_canvas_flips_and_reports() {
        use wordcartel_core::theme::{CanvasMode, Depth};
        // RGB theme at a color depth: flips + plain status.
        let mut ed = crate::editor::Editor::new_from_text("x", None, (40, 4));
        ed.theme = wordcartel_core::theme::Theme::builtin("flexoki-dark").unwrap();
        ed.depth = Depth::Truecolor;
        assert_eq!(ed.canvas, CanvasMode::Opaque);
        toggle_canvas(&mut ed);
        assert_eq!(ed.canvas, CanvasMode::Transparent);
        assert_eq!(ed.status, "canvas: transparent");
        // Non-Rgb theme: flips + persists, honest "no effect".
        let mut ed2 = crate::editor::Editor::new_from_text("x", None, (40, 4));
        ed2.theme = wordcartel_core::theme::Theme::builtin("terminal-plain").unwrap();
        ed2.depth = Depth::Truecolor;
        toggle_canvas(&mut ed2);
        assert_eq!(ed2.canvas, CanvasMode::Transparent, "flip persists even when inert");
        assert_eq!(ed2.status, "canvas: transparent (no effect: terminal-plain has no canvas)");
        // Depth::None (cue) on an Rgb theme: also "no effect" (no color to paint).
        let mut ed3 = crate::editor::Editor::new_from_text("x", None, (40, 4));
        ed3.theme = wordcartel_core::theme::Theme::builtin("flexoki-dark").unwrap();
        ed3.depth = Depth::None;
        toggle_canvas(&mut ed3);
        assert_eq!(ed3.canvas, CanvasMode::Transparent);
        assert_eq!(ed3.status, "canvas: transparent (no effect: flexoki-dark has no canvas)");
    }

    // -----------------------------------------------------------------------
    // Task 6 (E3+E4): toggle_chrome — honest arms + rederive flag
    // -----------------------------------------------------------------------

    #[test]
    fn toggle_chrome_flips_and_requests_rederive() {
        use wordcartel_core::theme::ChromeDisposition;
        let mut ed = Editor::new_from_text("x", None, (80, 24));
        assert_eq!(ed.chrome_disposition, ChromeDisposition::Full, "precondition: Full");
        dispatch_id(&mut ed, "toggle_chrome");
        assert_eq!(ed.chrome_disposition, ChromeDisposition::Zen, "disposition must flip to Zen");
        assert!(ed.theme_rederive, "rederive flag must be set");
        assert!(ed.status.contains("chrome: zen"), "status must say 'chrome: zen': {:?}", ed.status);
        // Second toggle flips back to Full.
        dispatch_id(&mut ed, "toggle_chrome");
        assert_eq!(ed.chrome_disposition, ChromeDisposition::Full, "second toggle → Full");
        assert!(ed.status.contains("chrome: full"), "status: {:?}", ed.status);
    }

    #[test]
    fn toggle_chrome_cue_mode_arm_no_flip() {
        // Arm 1: monochrome/cue theme — no flip, status "chrome: n/a (cue mode)".
        use wordcartel_core::theme::ChromeDisposition;
        let mut ed = Editor::new_from_text("x", None, (80, 24));
        ed.theme.monochrome = true;
        dispatch_id(&mut ed, "toggle_chrome");
        assert_eq!(ed.chrome_disposition, ChromeDisposition::Full, "cue mode: disposition must NOT flip");
        assert!(!ed.theme_rederive, "cue mode: rederive flag must NOT be set");
        assert_eq!(ed.status, "chrome: n/a (cue mode)", "cue mode status: {:?}", ed.status);
    }

    #[test]
    fn toggle_chrome_fixed_chrome_arm_flips_but_warns() {
        // Arm 2: non-Rgb bases (terminal-plain) — flips + persists, warns "no effect: {name}".
        use wordcartel_core::theme::{ChromeDisposition, Color};
        let mut ed = Editor::new_from_text("x", None, (80, 24));
        // new_from_text seeds terminal-plain (default()), which has non-Rgb bases.
        assert!(!matches!(ed.theme.base_bg, Color::Rgb { .. }), "precondition: non-Rgb base_bg");
        assert!(!ed.theme.monochrome, "precondition: not monochrome");
        dispatch_id(&mut ed, "toggle_chrome");
        assert_eq!(ed.chrome_disposition, ChromeDisposition::Zen, "fixed-chrome arm: disposition flips");
        assert!(ed.theme_rederive, "rederive flag set (though rederive is a no-op on non-Rgb)");
        assert!(ed.status.contains("no effect:"), "must warn 'no effect': {:?}", ed.status);
        assert!(ed.status.contains("has fixed chrome"), "must say 'has fixed chrome': {:?}", ed.status);
    }

    #[test]
    fn toggle_chrome_ansi16_arm_flips_but_warns() {
        // Arm 3: Rgb theme at Ansi16 depth — flips + persists, warns "no effect at 16-color depth".
        use wordcartel_core::theme::{ChromeDisposition, Depth};
        let mut ed = Editor::new_from_text("x", None, (80, 24));
        // Install an Rgb theme (flexoki-dark) and set depth to Ansi16.
        let theme = wordcartel_core::theme::Theme::builtin("flexoki-dark").unwrap();
        ed.apply_theme(theme);
        ed.depth = Depth::Ansi16;
        dispatch_id(&mut ed, "toggle_chrome");
        assert_eq!(ed.chrome_disposition, ChromeDisposition::Zen, "Ansi16 arm: disposition flips");
        assert!(ed.theme_rederive, "rederive flag must be set");
        assert!(ed.status.contains("no effect at 16-color depth"),
            "must warn 16-color: {:?}", ed.status);
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
        assert!(!ed.active().folds.folded().contains(&a_byte),
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
        assert!(!ed.active().folds.folded().contains(&a_byte),
            "## A fold must be cleared when diag_prev lands inside its body");
    }

    // -----------------------------------------------------------------------
    // Task 7: state-in-label menu items (MenuMark + CommandMeta.state)
    // -----------------------------------------------------------------------

    #[test]
    fn stateful_commands_report_live_state() {
        let reg = Registry::builtins();
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
        ed.view_opts.word_count = false;
        let m = reg.meta(CommandId("toggle_word_count")).unwrap();
        let f = m.state.expect("toggle_word_count has a state fn");
        assert!(matches!(f(&ed), MenuMark::OnOff(false)));
        ed.view_opts.word_count = true;
        assert!(matches!(f(&ed), MenuMark::OnOff(true)));
        // Chrome is a Value mark.
        let cm = reg.meta(CommandId("toggle_chrome")).unwrap().state.unwrap();
        ed.chrome_disposition = wordcartel_core::theme::ChromeDisposition::Zen;
        assert!(matches!(cm(&ed), MenuMark::Value("Zen")));
    }

    #[test]
    fn keymap_group_collapses_to_one_cycle_row() {
        let reg = Registry::builtins();
        // keymap_cua/keymap_wordstar are palette-only now (menu: None).
        assert_eq!(reg.meta(CommandId("keymap_cua")).unwrap().menu, None);
        assert_eq!(reg.meta(CommandId("keymap_next")).unwrap().menu, Some(MenuCategory::Settings));
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
        assert!(ed.active().folds.folded().contains(&a));
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

    // -----------------------------------------------------------------------
    // A1 Task 2: menu_bar_pin round-trips mode and clears auto-mode state.
    // -----------------------------------------------------------------------

    /// Pinning from Auto sets Pinned and clears all three auto fields;
    /// a second dispatch restores Auto.
    #[test]
    fn pin_toggle_round_trips_and_clears_auto_state() {
        use crate::config::MenuBarMode;
        let mut ed = Editor::new_from_text("x\n", None, (80, 24));
        // Start in Auto mode with stale auto-state.
        ed.menu_bar_mode = MenuBarMode::Auto;
        ed.mouse.menu_bar_revealed = true;
        ed.mouse.menu_reveal_due = Some(9999);
        ed.mouse.menu_hide_due = Some(9999);

        // First dispatch: Auto → Pinned, all auto-state cleared.
        dispatch_id(&mut ed, "menu_bar_pin");
        assert_eq!(ed.menu_bar_mode, MenuBarMode::Pinned);
        assert!(!ed.mouse.menu_bar_revealed, "menu_bar_revealed must be cleared on pin");
        assert!(ed.mouse.menu_reveal_due.is_none(), "menu_reveal_due must be cleared on pin");
        assert!(ed.mouse.menu_hide_due.is_none(), "menu_hide_due must be cleared on pin");
        // unpinned_mode must have captured Auto so it can be restored.
        assert_eq!(ed.menu_bar_unpinned_mode, MenuBarMode::Auto);

        // Second dispatch: Pinned → Auto restored.
        dispatch_id(&mut ed, "menu_bar_pin");
        assert_eq!(ed.menu_bar_mode, MenuBarMode::Auto, "second pin must restore Auto");
    }

    // -----------------------------------------------------------------------
    // A3 Task 2: scrollbar/status_line/menu_bar option-reachability commands
    // -----------------------------------------------------------------------

    #[test]
    fn scrollbar_commands_set_and_cycle() {
        use crate::config::TransientMode;
        let reg = Registry::builtins();
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
        dispatch_id(&mut ed, "scrollbar_off"); assert_eq!(ed.scrollbar_mode, TransientMode::Off);
        dispatch_id(&mut ed, "cycle_scrollbar"); assert_eq!(ed.scrollbar_mode, TransientMode::Auto); // Off→Auto
        dispatch_id(&mut ed, "cycle_scrollbar"); assert_eq!(ed.scrollbar_mode, TransientMode::On);   // Auto→On
        // palette-only: the set commands are not in the menu
        assert_eq!(reg.meta(CommandId("scrollbar_off")).unwrap().menu, None);
        // the representative is a View menu command with state-in-label
        assert_eq!(reg.meta(CommandId("cycle_scrollbar")).unwrap().menu, Some(MenuCategory::View));
    }

    #[test]
    fn status_line_toggle_and_menu_bar_sets() {
        use crate::config::{TransientMode, MenuBarMode};
        let reg = Registry::builtins();
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
        ed.set_status_line_mode(TransientMode::Auto);
        dispatch_id(&mut ed, "toggle_status_line"); assert_eq!(ed.status_line_mode, TransientMode::On);
        dispatch_id(&mut ed, "toggle_status_line"); assert_eq!(ed.status_line_mode, TransientMode::Auto);
        dispatch_id(&mut ed, "menu_bar_hidden"); assert_eq!(ed.menu_bar_mode, MenuBarMode::Hidden);
        assert_eq!(reg.meta(CommandId("menu_bar_hidden")).unwrap().menu, None);
    }

    #[test]
    fn splash_commands_registered_with_contract_shape() {
        let reg = Registry::builtins();
        let meta = |id: &str| reg.meta(reg.resolve_name(id).expect(id)).expect(id);
        assert_eq!(meta("splash_on").menu, None, "set-per-state primitives are palette-only");
        assert_eq!(meta("splash_off").menu, None, "set-per-state primitives are palette-only");
        let t = meta("toggle_splash");
        assert_eq!(t.menu, Some(MenuCategory::View), "toggle is the stateful View representative");
        let e = Editor::new_from_text("hi\n", None, (80, 24));
        assert_eq!((t.state.expect("stateful"))(&e), MenuMark::OnOff(true),
            "the OnOff mark mirrors the live option (default on)");
    }

    #[test]
    fn splash_commands_move_view_opts_through_set_splash() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx };
        assert!(ctx.editor.view_opts.splash, "default on");
        let r = reg.dispatch(CommandId("splash_off"), &mut ctx);
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert!(!ctx.editor.view_opts.splash);
        assert!(ctx.editor.status.contains("next launch"),
            "status notes the deferred effect: {}", ctx.editor.status);
        reg.dispatch(CommandId("toggle_splash"), &mut ctx);
        assert!(ctx.editor.view_opts.splash, "toggle flips back on");
        reg.dispatch(CommandId("splash_on"), &mut ctx);
        assert!(ctx.editor.view_opts.splash, "absolute set is idempotent");
        reg.dispatch(CommandId("toggle_splash"), &mut ctx);
        assert!(!ctx.editor.view_opts.splash, "toggle flips off");
    }
}
