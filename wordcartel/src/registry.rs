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
    /// The filesystem seam. OWNED (`Arc`), not borrowed, because `jobs::Job::run` is
    /// `Box<dyn FnOnce() -> JobResult + Send>` — a job closure must be able to clone this in.
    /// Synchronous call sites still take plain `&dyn Fs`; see §5.2 of the C5 spec.
    pub fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
}

pub type Handler = fn(&mut Ctx) -> CommandResult;

/// A registered command's implementation: a built-in fn pointer, or a plugin (enqueue + pump).
/// `registry.rs` stays Lua-free — the `Plugin` arm carries no Lua-typed value; dispatch only
/// enqueues a [`crate::plugin::PluginCall`] (owned `arg: Option<String>`, Task 5 — no longer
/// `Copy`), and the pump is what runs Lua.
pub enum HandlerKind {
    Builtin(Handler),
    Plugin,
}

/// Why [`Registry::register_plugin`] failed. Inputs arrive pre-interned/pre-capped (Task 4's
/// load layer), so a collision with a builtin or an earlier plugin command is the only failure.
#[derive(Debug, PartialEq, Eq)]
pub enum RegisterError {
    Duplicate,
}

// ── Command metadata ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuCategory { File, Edit, Block, Format, View, Documents, Settings, Export }

pub const MENU_ORDER: [MenuCategory; 8] = [MenuCategory::File, MenuCategory::Edit,
    MenuCategory::Block, MenuCategory::Format, MenuCategory::View, MenuCategory::Documents,
    MenuCategory::Settings, MenuCategory::Export];

/// Parse a plugin-supplied `menu` string to a `MenuCategory` — the parse-to-enum half of the
/// resource-bound LAW (Effort P1 global constraint 1b): an unknown menu name never enters the
/// registry as free-form data, it is rejected as a typed error at the call site. Exhaustive
/// over the eight variants (`MENU_ORDER`); unrecognized input is `None`, never a silent default.
pub fn menu_from_str(s: &str) -> Option<MenuCategory> {
    match s {
        "File" => Some(MenuCategory::File),
        "Edit" => Some(MenuCategory::Edit),
        "Block" => Some(MenuCategory::Block),
        "Format" => Some(MenuCategory::Format),
        "View" => Some(MenuCategory::View),
        "Documents" => Some(MenuCategory::Documents),
        "Settings" => Some(MenuCategory::Settings),
        "Export" => Some(MenuCategory::Export),
        _ => None,
    }
}

/// The live-state mark a stateful menu command interpolates into its row label.
/// Exhaustive — adding a variant here is intentional and must be handled in every match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuMark { OnOff(bool), Value(&'static str), Text(String) }

#[derive(Clone, Copy)]
pub struct CommandMeta {
    pub label: &'static str,
    pub menu: Option<MenuCategory>,
    /// Optional live-state provider — evaluated at menu-build time against `&Editor`.
    /// `None` for stateless commands (their static label renders unchanged).
    pub state: Option<fn(&crate::editor::Editor) -> MenuMark>,
    /// `Some(prompt)` for a parameterized plugin command — dispatching it with no arg in hand
    /// opens a [`crate::minibuffer::MinibufferKind::PluginArg`] with this prompt (Task 5).
    /// `None` for every builtin (today) and every nullary plugin command.
    pub arg: Option<&'static str>,
    /// A17 T8: `true` iff dispatching this command would mutate the active buffer's content OR run
    /// a mutating epilogue (e.g. clearing `marked_block`) after a no-op'd edit. On a read-only
    /// buffer, `dispatch_with_arg` refuses such a command before its handler runs — closing the
    /// EPILOGUE residual. The set is defined mechanically by the completeness sweep test
    /// (`no_registry_command_runs_a_mutating_epilogue_on_a_read_only_buffer`), NOT a hand-list.
    pub mutates: bool,
}

// ── Registry ──────────────────────────────────────────────────────────────────

struct CommandEntry {
    id: CommandId,
    handler: HandlerKind,
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
        self.entries.push(CommandEntry { id: cid, handler: HandlerKind::Builtin(handler),
            meta: CommandMeta { label, menu, state: None, arg: None, mutates: false } });
    }

    /// A17 T8: register a command that mutates the active buffer's content or runs a mutating
    /// epilogue — `dispatch_with_arg` refuses it on a read-only buffer. Membership is driven by the
    /// completeness sweep test, never a hand-list.
    fn register_mut(&mut self, id: &'static str, label: &'static str, menu: Option<MenuCategory>, handler: Handler) {
        let cid = CommandId(id);
        self.index.insert(cid, self.entries.len());
        self.entries.push(CommandEntry { id: cid, handler: HandlerKind::Builtin(handler),
            meta: CommandMeta { label, menu, state: None, arg: None, mutates: true } });
    }

    fn register_stateful(&mut self, id: &'static str, label: &'static str, menu: Option<MenuCategory>,
                         state: fn(&crate::editor::Editor) -> MenuMark, handler: Handler) {
        let cid = CommandId(id);
        self.index.insert(cid, self.entries.len());
        self.entries.push(CommandEntry { id: cid, handler: HandlerKind::Builtin(handler),
            meta: CommandMeta { label, menu, state: Some(state), arg: None, mutates: false } });
    }

    /// Register a plugin command. Inputs are ALREADY interned `&'static` (the load layer capped
    /// and interned them, Task 4) — so the only failure here is a collision with a builtin or
    /// an earlier plugin command. Never leaks (interning happened upstream). `arg` is
    /// `Some(prompt)` for a parameterized command (Task 5) — `dispatch`/`dispatch_with_arg` opens
    /// the `PluginArg` minibuffer with this prompt when no arg is already in hand.
    pub fn register_plugin(&mut self, id: CommandId, label: &'static str, menu: Option<MenuCategory>,
        arg: Option<&'static str>) -> Result<(), RegisterError> {
        if self.index.contains_key(&id) {
            return Err(RegisterError::Duplicate);
        }
        self.index.insert(id, self.entries.len());
        self.entries.push(CommandEntry { id, handler: HandlerKind::Plugin,
            meta: CommandMeta { label, menu, state: None, arg, mutates: false } });
        Ok(())
    }

    /// Remove every `Plugin` entry, keeping builtins — the reload teardown's registry half (P2 §6b).
    /// Fully rebuilds `index` from the surviving `entries`: removing an interior entry shifts every
    /// later position, so the old indices are wholesale invalid — never patch them incrementally.
    ///
    /// # Examples
    /// ```
    /// # use wordcartel::registry::{Registry, CommandId};
    /// let mut r = Registry::builtins();
    /// r.register_plugin(CommandId("demo.hi"), "Hi", None, None).unwrap();
    /// r.retain_builtins();
    /// assert!(r.resolve_name("demo.hi").is_none());
    /// assert!(r.resolve_name("save").is_some());
    /// ```
    pub fn retain_builtins(&mut self) {
        // `matches!(&e.handler, …)` borrows the discriminant — HandlerKind is NOT Copy, so
        // `matches!(e.handler, …)` would try to move the field out of the `&CommandEntry` and fail.
        self.entries.retain(|e| matches!(&e.handler, HandlerKind::Builtin(_)));
        self.index.clear();
        for (i, e) in self.entries.iter().enumerate() {
            self.index.insert(e.id, i);
        }
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

        // Sentence motions (S5, Emacs M-a/M-e) — palette-only (menu: None).
        r.register("sentence_left",  "Move Sentence Left",  None, |c| run(c, Command::Move { dir: Dir::SentenceLeft,  extend: false }));
        r.register("sentence_right", "Move Sentence Right", None, |c| run(c, Command::Move { dir: Dir::SentenceRight, extend: false }));

        // Sentence selecting motions (extend) — palette-only (menu: None).
        r.register("select_sentence_left",  "Select Sentence Left",  None, |c| run(c, Command::Move { dir: Dir::SentenceLeft,  extend: true }));
        r.register("select_sentence_right", "Select Sentence Right", None, |c| run(c, Command::Move { dir: Dir::SentenceRight, extend: true }));

        // Paragraph / page / document navigation — palette-only (menu: None).
        r.register("move_paragraph_up",   "Move Paragraph Up",   None, |c| run(c, Command::Move { dir: Dir::ParagraphUp,   extend: false }));
        r.register("move_paragraph_down", "Move Paragraph Down", None, |c| run(c, Command::Move { dir: Dir::ParagraphDown, extend: false }));
        r.register("move_page_up",   "Move Page Up",   None, |c| run(c, Command::Move { dir: Dir::PageUp,   extend: false }));
        r.register("move_page_down", "Move Page Down", None, |c| run(c, Command::Move { dir: Dir::PageDown, extend: false }));
        r.register("move_screen_top",    "Move to Screen Top",    None, |c| run(c, Command::Move { dir: Dir::ScreenTop,    extend: false }));
        r.register("move_screen_bottom", "Move to Screen Bottom", None, |c| run(c, Command::Move { dir: Dir::ScreenBottom, extend: false }));
        r.register("move_doc_start", "Move to Start",  None, |c| run(c, Command::Move { dir: Dir::DocStart, extend: false }));
        r.register("move_doc_end",   "Move to End",    None, |c| run(c, Command::Move { dir: Dir::DocEnd,   extend: false }));

        // Word delete — keystroke-native atomic edits, palette-only (A3b).
        r.register("delete_word_back",    "Delete Word Left",  None, |c| run(c, Command::DeleteWord { back: true }));
        r.register("delete_word_forward", "Delete Word Right", None, |c| run(c, Command::DeleteWord { back: false }));
        r.register("delete_line",         "Delete Line",        None, |c| run(c, Command::DeleteLine));
        r.register("delete_to_line_end",  "Delete to Line End", None, |c| run(c, Command::DeleteToLineEnd));

        // Editing — palette-only (menu: None).
        r.register("insert_newline", "Insert Newline",   None, |c| run(c, Command::InsertNewline));
        r.register("backspace",      "Backspace",        None, |c| run(c, Command::Backspace));
        r.register("delete_forward", "Delete Forward",   None, |c| run(c, Command::DeleteForward));

        // A14 — ten Emacs-parity atomic text-edit commands (commands/textops.rs).
        // Registered BEFORE save_settings (Codex F4): journey_palette_end_reaches_last_command
        // + the registration-order invariant both rely on save_settings staying last.
        r.register("transpose_chars", "Transpose Characters", None, |c| crate::commands::textops::transpose_chars(c.editor, c.clock));
        r.register("transpose_words", "Transpose Words",      None, |c| crate::commands::textops::transpose_words(c.editor, c.clock));
        r.register("transpose_lines", "Transpose Lines",      None, |c| crate::commands::textops::transpose_lines(c.editor, c.clock));
        r.register("upcase",     "Uppercase",  Some(MenuCategory::Format), |c| crate::commands::textops::upcase(c.editor, c.clock));
        r.register("downcase",   "Lowercase",  Some(MenuCategory::Format), |c| crate::commands::textops::downcase(c.editor, c.clock));
        r.register("capitalize", "Capitalize", Some(MenuCategory::Format), |c| crate::commands::textops::capitalize(c.editor, c.clock));
        r.register("join_line",              "Join Line",              None, |c| crate::commands::textops::join_line(c.editor, c.clock));
        r.register("just_one_space",         "Just One Space",         None, |c| crate::commands::textops::just_one_space(c.editor, c.clock));
        r.register("delete_blank_lines",     "Delete Blank Lines",     None, |c| crate::commands::textops::delete_blank_lines(c.editor, c.clock));
        r.register("delete_horizontal_space","Delete Horizontal Space",None, |c| crate::commands::textops::delete_horizontal_space(c.editor, c.clock));

        // Edit menu.
        r.register("select_all", "Select All", Some(MenuCategory::Edit), |c| run(c, Command::SelectAll));
        r.register("copy",  "Copy",  Some(MenuCategory::Edit), |c| run(c, Command::Copy));
        r.register("cut",   "Cut",   Some(MenuCategory::Edit), |c| run(c, Command::Cut));
        r.register("paste", "Paste", Some(MenuCategory::Edit), |c| run(c, Command::Paste));
        r.register("undo",  "Undo",  Some(MenuCategory::Edit), |c| run(c, Command::Undo));
        r.register("redo",  "Redo",  Some(MenuCategory::Edit), |c| run(c, Command::Redo));
        // filter: Format menu (A3b) — a text-shaping op, sibling of reflow/unwrap/ventilate.
        r.register("filter", "Filter…", Some(MenuCategory::Format), |c| {
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
        // H5: clear provably-valueless recovery litter. Opens a count-confirm modal over a
        // snapshotted set (safe by construction — see prompts::open_clean_recovery). Trailing …
        // marks the prompt-opening command (cf. save_as); palette + File menu by registration.
        r.register("clean_recovery", "Clean Recovery Files\u{2026}", Some(MenuCategory::File), |c| {
            crate::prompts::open_clean_recovery(c.editor, &*c.fs);
            CommandResult::Handled
        });

        // View menu — render mode: set-per-state primitives (palette-only) + stateful cycle
        // representative, mirroring scrollbar_off/auto/on + cycle_scrollbar (contract law 6 / rule 8).
        r.register("view_live_preview",       "View: Live Preview",       None,
            |c| { c.editor.set_render_mode(crate::editor::RenderMode::LivePreview, c.clock.now_ms()); CommandResult::Handled });
        r.register("view_review",             "View: Review",             None,
            |c| { c.editor.set_render_mode(crate::editor::RenderMode::Review, c.clock.now_ms()); CommandResult::Handled });
        r.register("view_source_highlighted", "View: Source Highlighted", None,
            |c| { c.editor.set_render_mode(crate::editor::RenderMode::SourceHighlighted, c.clock.now_ms()); CommandResult::Handled });
        r.register("view_source_plain",       "View: Source Plain",       None,
            |c| { c.editor.set_render_mode(crate::editor::RenderMode::SourcePlain, c.clock.now_ms()); CommandResult::Handled });
        r.register_stateful("cycle_render_mode", "Render Mode", Some(MenuCategory::View),
            |e| MenuMark::Value(match e.active().view.mode {
                crate::editor::RenderMode::LivePreview       => "Live",
                crate::editor::RenderMode::Review            => "Review",
                crate::editor::RenderMode::SourceHighlighted => "SRC-HI",
                crate::editor::RenderMode::SourcePlain       => "Source" }),
            |c| run(c, Command::CycleRenderMode));
        // transform: palette-only (A16) — the discrete variants (reflow/unwrap/ventilate) already
        // carry the Format door, so the Transform… umbrella row is redundant. Command stays
        // registered + palette-reachable; Ctrl-T (input.rs/keymap.rs) is unaffected.
        r.register("transform", "Transform…", None, |c| {
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
        r.register("select_section",   "Select Section",   None, |c| run(c, Command::SelectScope(Scope::Section)));
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
            let was_open = c.editor.menu.is_some();
            crate::overlays::close_all(c.editor);
            c.editor.pending_keys.clear();
            c.editor.pending_mark = None;
            c.editor.menu = if was_open { None } else { Some(crate::menu::empty()) };
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
                          c.editor.set_status(crate::status::StatusKind::Info, concat!("bookmark ", $d, " set").to_string());
                          CommandResult::Handled });
                $r.register(concat!("jump_bookmark_", $d), concat!("Jump to Bookmark ", $d), None,
                    |c| { if crate::marks::jump_char_mark(c.editor, $ch) {
                              c.editor.set_status(crate::status::StatusKind::Info, concat!("jumped to bookmark ", $d).to_string());
                          } else {
                              c.editor.set_status(crate::status::StatusKind::Info, concat!("no bookmark ", $d).to_string());
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
        r.register("block_begin",               "Set Block Begin",         Some(MenuCategory::Block), |c| { crate::blocks_marked::block_begin(c.editor); CommandResult::Handled });
        r.register("block_end",                 "Set Block End",           Some(MenuCategory::Block), |c| { crate::blocks_marked::block_end(c.editor); CommandResult::Handled });
        r.register_mut("mark_block_from_selection", "Mark Block from Selection", Some(MenuCategory::Block), |c| { crate::blocks_marked::mark_block_from_selection(c.editor); CommandResult::Handled });
        // Block → selection bridge (A11.3, Task 1.1 / command-surface curation).
        r.register("select_marked_block", "Select Block", Some(MenuCategory::Block),
            |c| { crate::blocks_marked::select_marked_block(c.editor); CommandResult::Handled });
        // Two-region swap (S4 Task 6): exchange Selection <-> MarkedBlock, one undo unit.
        r.register_mut("swap", "Swap Selection \u{21C4} Block", Some(MenuCategory::Block),
            |c| crate::commands::prose_ops::swap(c.editor, c.clock));

        // Marked block operations (Task 3 / Effort 9A).
        r.register("block_copy",          "Copy Block",        Some(MenuCategory::Block), |c| { crate::blocks_marked::block_copy(c.editor, c.clock);   CommandResult::Handled });
        r.register_mut("block_move",          "Move Block",        Some(MenuCategory::Block), |c| { crate::blocks_marked::block_move(c.editor, c.clock);   CommandResult::Handled });
        r.register_mut("block_delete",        "Delete Block",      Some(MenuCategory::Block), |c| { crate::blocks_marked::block_delete(c.editor, c.clock); CommandResult::Handled });
        r.register("block_jump_begin",    "Jump to Block Begin", Some(MenuCategory::Block), |c| { crate::blocks_marked::block_jump_begin(c.editor);    CommandResult::Handled });
        r.register("block_jump_end",      "Jump to Block End",   Some(MenuCategory::Block), |c| { crate::blocks_marked::block_jump_end(c.editor);      CommandResult::Handled });
        r.register_mut("block_toggle_hidden", "Toggle Block Hidden", Some(MenuCategory::Block), |c| { crate::blocks_marked::block_toggle_hidden(c.editor); CommandResult::Handled });
        r.register_mut("block_clear",         "Clear Block",         Some(MenuCategory::Block), |c| { crate::blocks_marked::block_clear(c.editor);         CommandResult::Handled });
        // Marked block write-to-file (Task 4 / Effort 9A).
        r.register("block_write", "Write Block to File\u{2026}", Some(MenuCategory::Block), |c| { crate::blocks_marked::block_write(c.editor); CommandResult::Handled });

        // Effort 6: send-to-scratch verbs.
        r.register("copy_block_to_scratch", "Copy Block to Scratch", Some(MenuCategory::Block), |c| { crate::scratch::copy_block_to_scratch(c.editor, c.clock); CommandResult::Handled });
        r.register_mut("move_block_to_scratch", "Move Block to Scratch", Some(MenuCategory::Block), |c| { crate::scratch::move_block_to_scratch(c.editor, c.clock); CommandResult::Handled });

        // Effort 6: workspace navigation. next_buffer/prev_buffer/switch_buffer are
        // palette-only (menu: None) as of Task 4.2 — the Documents dynamic menu's direct
        // per-buffer rows make them duplicative on the menu surface only; they keep their
        // registered-command status, palette listing, and keymap chords.
        r.register("next_buffer", "Next Buffer", None, |c| { crate::workspace::next_buffer(c.editor); CommandResult::Handled });
        r.register("prev_buffer", "Previous Buffer", None, |c| { crate::workspace::prev_buffer(c.editor); CommandResult::Handled });
        r.register("view_messages", "Message History", Some(MenuCategory::View), |c| { crate::status_view::open(c.editor); CommandResult::Handled });
        r.register("goto_scratch", "Go to Scratch Buffer", Some(MenuCategory::View), |c| { crate::workspace::goto_scratch(c.editor); CommandResult::Handled });
        r.register("toggle_scratch", "Toggle Scratch Buffer", Some(MenuCategory::View), |c| { crate::workspace::toggle_scratch(c.editor); CommandResult::Handled });
        r.register("switch_buffer", "Switch Buffer\u{2026}", None, |c| { c.editor.open_buffer_switcher(); CommandResult::Handled });
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
        // Each handler reads through active_lens_diags (Task 6): whichever engine is the active
        // lens, gated on Review/show + that slot's version validity — the single source of truth
        // for "what the lens shows".
        r.register("quick_fix", "Quick Fix\u{2026}", None, |c| {
            let Some(diags) = crate::diagnostics_run::active_lens_diags(c.editor) else {
                c.editor.set_status(crate::status::StatusKind::Info, "no diagnostic here");
                return CommandResult::Handled;
            };
            let caret = c.editor.active().document.selection.primary().head;
            let diag = diags.iter()
                .find(|d| d.range.start <= caret && caret <= d.range.end)
                .cloned();
            if let Some(d) = diag {
                c.editor.open_diag(d);
            } else {
                c.editor.set_status(crate::status::StatusKind::Info, "no diagnostic here");
            }
            CommandResult::Handled
        });
        r.register("diag_next", "Next Diagnostic", None, |c| {
            let Some(diags) = crate::diagnostics_run::active_lens_diags(c.editor) else {
                return CommandResult::Handled;
            };
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
            let Some(diags) = crate::diagnostics_run::active_lens_diags(c.editor) else {
                return CommandResult::Handled;
            };
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
            if crate::diagnostics_run::should_run_diagnostics(c.editor) {
                crate::diagnostics_run::arm_enabled(c.editor, c.clock.now_ms(), 0);
            }
            CommandResult::Handled
        });

        // Analysis lens — set-per-state primitive (palette-only) + stateful cycle representative
        // (contract rule 8 — the keymap_next / cycle_render_mode precedent). One primitive per
        // AVAILABLE core engine; the ltex/vale effort adds its siblings here.
        r.register("analysis_engine_harper", "Analysis Engine: Harper", None, |c| {
            c.editor.set_analysis_source(wordcartel_core::diagnostics::DiagSource::Harper);
            CommandResult::Handled
        });
        r.register_stateful("analysis_next", "Analysis Engine", Some(MenuCategory::View),
            |e| MenuMark::Value(e.active_analysis_source.label()),
            |c| { crate::diagnostics_run::cycle_analysis_source(c.editor); CommandResult::Handled });
        // Per-engine enablement — a 2-state toggle (contract rule 8), palette-only.
        r.register("toggle_engine_harper", "Toggle Harper Engine", None, |c| {
            let on = !c.editor.diag_providers.is_enabled(wordcartel_core::diagnostics::DiagSource::Harper);
            crate::diagnostics_run::set_engine_enabled(c.editor,
                wordcartel_core::diagnostics::DiagSource::Harper, on, c.clock);
            CommandResult::Handled
        });

        // Prose lenses (S8) — Rule 8: 5 palette-only set primitives + one stateful cycle rep; the
        // shared setter is lenses::set_prose_lens (Law 6). Per-buffer state on View. A14: no
        // Command variant, no commands::run arm — thin delegations into the lenses leaf.
        r.register("prose_lens_adverbs",    "Prose Lens: Adverbs",    None, |c| { crate::lenses::set_prose_lens(c.editor, Some(crate::lenses::ProseLensCategory::Adverbs)); CommandResult::Handled });
        r.register("prose_lens_adjectives", "Prose Lens: Adjectives", None, |c| { crate::lenses::set_prose_lens(c.editor, Some(crate::lenses::ProseLensCategory::Adjectives)); CommandResult::Handled });
        r.register("prose_lens_passive",    "Prose Lens: Passive",    None, |c| { crate::lenses::set_prose_lens(c.editor, Some(crate::lenses::ProseLensCategory::Passive)); CommandResult::Handled });
        r.register("prose_lens_weak",       "Prose Lens: Weak",       None, |c| { crate::lenses::set_prose_lens(c.editor, Some(crate::lenses::ProseLensCategory::Weak)); CommandResult::Handled });
        r.register("prose_lens_off",        "Prose Lens: Off",        None, |c| { crate::lenses::set_prose_lens(c.editor, None); CommandResult::Handled });
        r.register_stateful("prose_lens_next", "Prose Lens", Some(MenuCategory::View),
            |e| match e.active().view.prose_lens {
                Some(cat) => MenuMark::Value(crate::lenses::category_label(cat)),
                None => MenuMark::Value("Off"),
            },
            |c| { crate::lenses::cycle_prose_lens(c.editor); CommandResult::Handled });
        // Nav — two-side pair (contract rule): jump the caret to the next/previous match under the
        // active lens, range-selecting the WHOLE matched span (C-9, D6) so it reads as a real find.
        r.register("prose_lens_next_match", "Next Prose-Lens Match", None, |c| { crate::lenses::prose_lens_next_match(c.editor); CommandResult::Handled });
        r.register("prose_lens_prev_match", "Previous Prose-Lens Match", None, |c| { crate::lenses::prose_lens_prev_match(c.editor); CommandResult::Handled });

        // View menu — writing-experience toggles (Task 2 / Effort 5d).
        r.register("toggle_typewriter", "Toggle Typewriter", Some(MenuCategory::View), |c| { c.editor.view_opts.typewriter = !c.editor.view_opts.typewriter; CommandResult::Handled });
        r.register("toggle_focus",      "Toggle Focus Mode", Some(MenuCategory::View), |c| { c.editor.view_opts.focus = !c.editor.view_opts.focus; CommandResult::Handled });
        r.register_stateful("toggle_measure", "Toggle Centered Measure", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.measure),
            |c| { c.editor.view_opts.measure = !c.editor.view_opts.measure; crate::derive::rebuild(c.editor); CommandResult::Handled });
        // Ventilate lens (S6) — per-buffer, so the state fn reads the ACTIVE buffer's view (unlike
        // the view_opts-global toggles above). set_ventilate is the ONE shared setter (Law 6).
        r.register_stateful("toggle_ventilate", "Toggle Ventilate View", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.active().view.ventilate),
            |c| { crate::ventilate::set_ventilate(c.editor, !c.editor.active().view.ventilate); CommandResult::Handled });
        r.register_stateful("toggle_wrap_guide", "Toggle Wrap Guide", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.wrap_guide),
            |c| { c.editor.view_opts.wrap_guide = !c.editor.view_opts.wrap_guide; CommandResult::Handled });
        r.register_stateful("toggle_word_count", "Toggle Word Count", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.word_count),
            |c| { c.editor.view_opts.word_count = !c.editor.view_opts.word_count; CommandResult::Handled });
        r.register("count_region", "Count Region", Some(MenuCategory::View),
            |c| crate::commands::prose_ops::count_region(c.editor));
        r.register("move_sentence_up",   "Move Sentence Up",   Some(MenuCategory::Edit), |c| crate::commands::prose_ops::move_sentence_up(c.editor, c.clock));
        r.register("move_sentence_down", "Move Sentence Down", Some(MenuCategory::Edit), |c| crate::commands::prose_ops::move_sentence_down(c.editor, c.clock));
        r.register("break_paragraph_here",    "Break Paragraph Here",    Some(MenuCategory::Edit), |c| crate::commands::prose_ops::break_paragraph_here(c.editor, c.clock));
        r.register("merge_paragraph_forward", "Merge Paragraph Forward", Some(MenuCategory::Edit), |c| crate::commands::prose_ops::merge_paragraph_forward(c.editor, c.clock));
        r.register("split_sentence_at_caret", "Split Sentence",          Some(MenuCategory::Edit), |c| crate::commands::prose_ops::split_sentence_at_caret(c.editor, c.clock));

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
                c.editor.set_status(crate::status::StatusKind::Info, "no heading at cursor");
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

        // Message verbosity floor (Q6, A17 T10): set-per-state (palette-only) + 2-state toggle
        // representative (View, state-in-label). All three route through the single
        // `Editor::set_messages_min_kind` setter (contract law 6).
        r.register("messages_min_info", "Messages: Info & Above", None, |c| {
            c.editor.set_messages_min_kind(crate::status::StatusKind::Info); CommandResult::Handled });
        r.register("messages_min_warning", "Messages: Warnings & Errors Only", None, |c| {
            c.editor.set_messages_min_kind(crate::status::StatusKind::Warning); CommandResult::Handled });
        r.register_stateful("toggle_messages_verbosity", "Message Verbosity", Some(MenuCategory::View),
            |e| MenuMark::Value(match e.messages_min_kind() {
                crate::status::StatusKind::Warning => "Warnings & Errors Only", _ => "Info & Above" }),
            |c| { let next = if c.editor.messages_min_kind() == crate::status::StatusKind::Warning {
                      crate::status::StatusKind::Info } else { crate::status::StatusKind::Warning };
                  c.editor.set_messages_min_kind(next); CommandResult::Handled });

        // Startup splash: set-per-state (palette-only) + 2-state toggle representative
        // (View, OnOff mark). All three route through Editor::set_splash (contract law 6);
        // the splash paints only at launch, so a change takes effect on the NEXT run.
        r.register("splash_on",  "Splash: On",  None, |c| { c.editor.set_splash(true);
            c.editor.set_status(crate::status::StatusKind::Info, "splash: on (takes effect next launch)"); CommandResult::Handled });
        r.register("splash_off", "Splash: Off", None, |c| { c.editor.set_splash(false);
            c.editor.set_status(crate::status::StatusKind::Info, "splash: off (takes effect next launch)"); CommandResult::Handled });
        r.register_stateful("toggle_splash", "Startup Splash", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.view_opts.splash),
            |c| { let next = !c.editor.view_opts.splash; c.editor.set_splash(next);
                  c.editor.set_status(crate::status::StatusKind::Info, if next { "splash: on (takes effect next launch)" }
                                    else { "splash: off (takes effect next launch)" });
                  CommandResult::Handled });

        // Caret shape: set-per-state (palette-only) + 4-state cycle representative (View, state-in-label).
        use crate::config::CaretShape;
        r.register("caret_shape_default",   "Caret Shape: Default",   None, |c| { c.editor.set_caret_shape(CaretShape::Default);   CommandResult::Handled });
        r.register("caret_shape_block",     "Caret Shape: Block",     None, |c| { c.editor.set_caret_shape(CaretShape::Block);     CommandResult::Handled });
        r.register("caret_shape_beam",      "Caret Shape: Beam",      None, |c| { c.editor.set_caret_shape(CaretShape::Beam);      CommandResult::Handled });
        r.register("caret_shape_underline", "Caret Shape: Underline", None, |c| { c.editor.set_caret_shape(CaretShape::Underline); CommandResult::Handled });
        r.register_stateful("cycle_caret_shape", "Caret Shape", Some(MenuCategory::View),
            |e| MenuMark::Value(match e.caret_shape {
                CaretShape::Default => "Default", CaretShape::Block => "Block",
                CaretShape::Beam => "Beam", CaretShape::Underline => "Underline" }),
            |c| { let next = match c.editor.caret_shape {
                      CaretShape::Default => CaretShape::Block, CaretShape::Block => CaretShape::Beam,
                      CaretShape::Beam => CaretShape::Underline, CaretShape::Underline => CaretShape::Default };
                  c.editor.set_caret_shape(next); CommandResult::Handled });

        // Caret blink: set-per-state (palette-only) + 2-state toggle representative (View, OnOff mark).
        r.register("caret_blink_on",  "Caret Blink: On",  None, |c| { c.editor.set_caret_blink(true);  CommandResult::Handled });
        r.register("caret_blink_off", "Caret Blink: Off", None, |c| { c.editor.set_caret_blink(false); CommandResult::Handled });
        r.register_stateful("toggle_caret_blink", "Caret Blink", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.caret_blink),
            |c| { let n = !c.editor.caret_blink; c.editor.set_caret_blink(n); CommandResult::Handled });

        // Caret picker: opens the live sample-cell shape/blink overlay (View).
        r.register("cursor", "Caret\u{2026}", Some(MenuCategory::View), |c| {
            c.editor.open_cursor_picker(); CommandResult::Handled });

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
        r.register_stateful("set_wrap_column", "Wrap Column: Set\u{2026}", Some(MenuCategory::Settings),
            |e| MenuMark::Text(format!("{}\u{2026}", e.view_opts.wrap_column)),
            |c| {
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
        // Plugin lifecycle (P2 §6). Both are real Settings commands (command-surface contract:
        // palette-exhaustive by construction, Settings menu by derivation). `plugins_reload` only
        // arms the request flag — the run loop's between-reduces reload seam
        // (plugin::reload::perform_reload) does the whole-VM teardown+rebuild, never inline here.
        r.register("plugins_reload", "Reload Plugins", Some(MenuCategory::Settings), |c| {
            c.editor.plugins_reload_requested = true;
            c.editor.set_status(crate::status::StatusKind::Info, "reloading plugins\u{2026}");
            CommandResult::Handled
        });
        r.register("plugin_list", "List Plugins", Some(MenuCategory::Settings), |c| {
            let inv = &c.editor.plugin_inventory;
            let ok = inv.iter().filter(|r| r.error.is_none()).count();
            let failed = inv.len() - ok;
            let cmds: usize = inv.iter().map(|r| r.commands).sum();
            let hooks: usize = inv.iter().map(|r| r.hooks).sum(); // real hook total (Task 6 wiring)
            let timers = c.editor.pending_plugin_timers.len(); // P3: live armed-timer count
            c.editor.set_status(crate::status::StatusKind::Info, format!(
                "plugins: {ok} ok ({cmds} cmds, {hooks} hooks, {timers} timers), {failed} failed"));
            CommandResult::Handled
        });

        r
    }

    /// Dispatch `id` with NO argument supplied — palette/keybinding/menu path (the existing
    /// callers, unchanged by Task 5). A parameterized plugin command (`meta.arg == Some`) that
    /// reaches here with no arg opens its prompt.
    pub fn dispatch(&self, id: CommandId, ctx: &mut Ctx) -> CommandResult {
        self.dispatch_with_arg(id, ctx, None)
    }

    /// Dispatch `id`, threading an OPTIONAL already-collected argument. `arg == Some` means the
    /// value is in hand (from `wc.command(name, arg)` or a resolved `PluginArg` prompt) —
    /// enqueue directly, NEVER re-prompt. Unknown ids surface a status (never a silent no-op,
    /// §12.5). Covers all four cases:
    /// 1. Builtin — nullary; any supplied arg is dropped (builtins take no arg today).
    /// 2. Plugin, arg SUPPLIED (wc.command with arg, or the user already answered the prompt) →
    ///    enqueue directly, no minibuffer. (Also covers a nullary plugin command that a plugin
    ///    passed an arg to — the callback simply ignores the extra value.)
    /// 3. Plugin DECLARES an arg (`meta.arg == Some`) but none supplied (palette/keybinding) →
    ///    open the `PluginArg` prompt; its submit re-enters via case 2's direct enqueue.
    /// 4. Nullary plugin command, no arg → enqueue nullary (today's behavior).
    pub fn dispatch_with_arg(&self, id: CommandId, ctx: &mut Ctx, arg: Option<String>) -> CommandResult {
        match self.index.get(&id) {
            Some(&i) => {
                // A17 T8 EPILOGUE residual: refuse a mutating command on a read-only buffer BEFORE
                // its handler runs, so no mutating epilogue (e.g. clearing `marked_block`) fires.
                // Content + status are already covered by categories (a)/(b) + the delegators.
                if self.entries[i].meta.mutates && ctx.editor.active().read_only {
                    ctx.editor.reject_read_only();
                    return CommandResult::Noop;
                }
                match &self.entries[i].handler {
                HandlerKind::Builtin(h) => { let _ = arg; h(ctx) }
                HandlerKind::Plugin => {
                    match (self.entries[i].meta.arg, arg) {
                        (_, Some(supplied)) => ctx.editor.pending_plugin_calls.push_back(
                            crate::plugin::PluginCall { id, arg: Some(supplied) }),
                        (Some(prompt), None) => {
                            // Hold the single-overlay XOR invariant (H21): the plugin pump drains
                            // this path UNCONDITIONALLY, so a plugin timer/event can open this
                            // PluginArg prompt while another overlay is already active. Mirror
                            // `Editor::open_minibuffer`'s close_all + pending clears — but NOT its
                            // `prompt.is_none()` debug_assert, which a plugin-triggered dispatch
                            // fired under a modal Prompt would trip before close_all could clear it.
                            crate::overlays::close_all(ctx.editor);
                            ctx.editor.pending_keys.clear();
                            ctx.editor.pending_mark = None;
                            ctx.editor.minibuffer = Some(crate::minibuffer::Minibuffer {
                                prompt: prompt.to_string(), text: String::new(), cursor: 0,
                                kind: crate::minibuffer::MinibufferKind::PluginArg { id },
                            });
                        }
                        (None, None) => ctx.editor.pending_plugin_calls.push_back(
                            crate::plugin::PluginCall { id, arg: None }),
                    }
                    CommandResult::Handled
                }
                }
            },
            None => {
                ctx.editor.set_status(crate::status::StatusKind::Info, format!("unknown command: {}", id.0));
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
        editor.set_status(crate::status::StatusKind::Info, format!("keymap: {preset} (already active)"));
        return;
    }
    editor.active_keymap_preset = preset.to_string();
    editor.keymap_rebuild = true;
    editor.set_status(crate::status::StatusKind::Info, format!("keymap: {preset}"));
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
        editor.set_status(crate::status::StatusKind::Info, "chrome: n/a (cue mode)");
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
        editor.set_status(crate::status::StatusKind::Info, format!("chrome: {label} (no effect: {name} has fixed chrome)"));
        return;
    }
    // Arm 3 — Rgb theme at Ansi16 depth: the fixed 5-face Ansi16 policy applied by
    // resolve_theme overrides the derived faces; toggling disposition has no visible effect.
    if editor.depth == Depth::Ansi16 {
        editor.set_status(crate::status::StatusKind::Info, format!("chrome: {label} (no effect at 16-color depth)"));
        return;
    }
    // Normal arm: derived Rgb theme at Truecolor/256; the rederive will visibly change chrome.
    editor.set_status(crate::status::StatusKind::Info, format!("chrome: {label}"));
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
        editor.set_status(crate::status::StatusKind::Info, format!("canvas: {label} (no effect: {name} has no canvas)"));
        return;
    }
    editor.set_status(crate::status::StatusKind::Info, format!("canvas: {label}"));
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
        c.editor.set_status(crate::status::StatusKind::Info, "no heading");
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

/// After a fold-set change that may have left the caret on a now-hidden line — the block-move / swap
/// fold-correction paths (S4) — snap the caret OUT of any fold and re-scroll, exactly the guard the
/// shipped fold commands (Undo/Redo, jump) apply. Assumes the tree is already rebuilt against the
/// corrected fold set, so `SnapOut`'s `normalize_caret` sees the final folds. Without this a
/// `block_move`/`swap` of a folded section can leave the head on a hidden line, where typing would
/// edit invisible text (Fable/Codex must-fix).
pub(crate) fn snap_caret_out_of_fold(editor: &mut crate::editor::Editor) {
    let head = editor.active().document.selection.primary().head;
    let snapped = place_caret_visible(editor, head, CaretPlace::SnapOut);
    if snapped != head {
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(snapped);
    }
    crate::nav::ensure_visible(editor);
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

    /// The compile-shape guard for this task: `Ctx.fs` must be an OWNED handle so a job
    /// closure (`'static + Send`, per `jobs::Job::run`) can clone it in and use it after
    /// crossing a real thread boundary — a borrowed `&dyn Fs` cannot do this.
    #[test]
    fn ctx_fs_field_exists_and_is_clonable_into_a_closure() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let ex = crate::jobs::InlineExecutor::default();
        let clk = crate::test_support::TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let ctx = Ctx {
            editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx,
            fs: std::sync::Arc::new(crate::fsx::RealFs),
        };
        let handle = std::sync::Arc::clone(&ctx.fs);
        let t = std::thread::spawn(move || handle.stat(std::path::Path::new("/")).is_ok());
        assert!(t.join().expect("joins"));
    }

    #[test]
    fn menu_from_str_parses_all_eight_and_rejects_unknown() {
        for (s, m) in [
            ("File", MenuCategory::File), ("Edit", MenuCategory::Edit),
            ("Block", MenuCategory::Block), ("Format", MenuCategory::Format),
            ("View", MenuCategory::View), ("Documents", MenuCategory::Documents),
            ("Settings", MenuCategory::Settings), ("Export", MenuCategory::Export),
        ] {
            assert_eq!(menu_from_str(s), Some(m));
        }
        assert_eq!(menu_from_str("Nonsense"), None);
        assert_eq!(menu_from_str(""), None);
    }

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
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch(CommandId("save"), &mut ctx);
        // No path → save handler opens the Save-As minibuffer (Effort 7, Task 3).
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert!(matches!(e.minibuffer.as_ref().map(|m| m.kind),
            Some(crate::minibuffer::MinibufferKind::SaveAs)),
            "unnamed save opens the Save-As minibuffer");
    }

    #[test]
    fn sentence_motion_commands_dispatch_and_take_effect() {
        // "One two. Three four." spans: (0,8), (9,20).
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        // Dispatch `id` against editor `e` with the caret preset to `caret`; return the new head
        // (and leave `e`'s selection for the caller to inspect).
        let dispatch = |e: &mut Editor, id: &'static str| {
            let mut ctx = Ctx { editor: e, clock: &clk, executor: &ex, msg_tx: tx.clone(), fs: crate::test_support::test_fs() };
            reg.dispatch(CommandId(id), &mut ctx)
        };
        let head = |e: &Editor| e.active().document.selection.primary().head;

        // sentence_left: caret in "Three four." → start of that sentence (9).
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(12);
        crate::derive::rebuild(&mut e);
        assert_eq!(dispatch(&mut e, "sentence_left"), CommandResult::Handled);
        assert_eq!(head(&e), 9);

        // sentence_right: caret at 0 → content end of first sentence (8).
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        crate::derive::rebuild(&mut e);
        assert_eq!(dispatch(&mut e, "sentence_right"), CommandResult::Handled);
        assert_eq!(head(&e), 8);

        // select_sentence_right: extends from anchor 0 → selection (0,8).
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        crate::derive::rebuild(&mut e);
        assert_eq!(dispatch(&mut e, "select_sentence_right"), CommandResult::Handled);
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (0, 8));

        // select_sentence_left: caret in "Three four." extends back to that sentence's start (9,12).
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(12);
        crate::derive::rebuild(&mut e);
        assert_eq!(dispatch(&mut e, "select_sentence_left"), CommandResult::Handled);
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (9, 12));
    }

    #[test]
    fn register_plugin_adds_a_dispatchable_command() {
        let mut reg = Registry::builtins();
        let id = CommandId(crate::plugin::intern("register-plugin-test.hello"));
        let label = crate::plugin::intern("Hello Plugin");
        reg.register_plugin(id, label, None, None).expect("register_plugin should succeed on a fresh id");
        assert_eq!(reg.resolve_name("register-plugin-test.hello"), Some(id));
        assert_eq!(reg.meta(id).unwrap().label, "Hello Plugin");
        assert!(reg.commands().any(|(cid, _)| cid == id), "must appear in commands()");
    }

    #[test]
    fn register_plugin_rejects_collision() {
        let mut reg = Registry::builtins();
        // Collides with a builtin.
        let err = reg.register_plugin(CommandId("save"), "Whatever", None, None).unwrap_err();
        assert_eq!(err, RegisterError::Duplicate);

        // Collides with a prior plugin registration; registry unchanged by the rejected call.
        let id = CommandId(crate::plugin::intern("register-plugin-test.dup"));
        reg.register_plugin(id, "Once", None, None).expect("first registration should succeed");
        let count_before = reg.commands().count();
        let err2 = reg.register_plugin(id, "Twice", None, None).unwrap_err();
        assert_eq!(err2, RegisterError::Duplicate);
        assert_eq!(reg.commands().count(), count_before, "registry unchanged by a rejected collision");
    }

    #[test]
    fn plugin_dispatch_enqueues_not_runs() {
        let mut reg = Registry::builtins();
        let id = CommandId(crate::plugin::intern("register-plugin-test.enqueue"));
        reg.register_plugin(id, "Enqueue Test", None, None).expect("register_plugin should succeed");

        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch(id, &mut ctx);

        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.pending_plugin_calls.len(), 1, "dispatch enqueues, does not run any Lua");
        assert_eq!(e.pending_plugin_calls[0], crate::plugin::PluginCall { id, arg: None });
    }

    // ── Task 5: the four-case `dispatch_with_arg` matrix ──────────────────────────────────

    /// Case 3 — a `Plugin` entry with `meta.arg = Some("Prompt")`, dispatched via `dispatch`
    /// (no arg supplied): opens a `PluginArg { id }` minibuffer with that prompt; nothing is
    /// enqueued yet.
    #[test]
    fn param_command_no_arg_opens_prompt() {
        let mut reg = Registry::builtins();
        let id = CommandId(crate::plugin::intern("param-cmd-test.no-arg"));
        reg.register_plugin(id, "Param Cmd", None, Some("Minutes:"))
            .expect("register_plugin should succeed on a fresh id");

        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch(id, &mut ctx);

        assert_eq!(r, CommandResult::Handled);
        assert!(e.pending_plugin_calls.is_empty(), "nothing enqueued until the prompt is answered");
        match e.minibuffer.as_ref().map(|m| (&m.prompt, &m.kind)) {
            Some((prompt, crate::minibuffer::MinibufferKind::PluginArg { id: pid })) => {
                assert_eq!(prompt, "Minutes:");
                assert_eq!(*pid, id);
            }
            other => panic!("expected a PluginArg minibuffer, got {other:?}"),
        }
    }

    /// Case 2 (the bug this fixes) — `dispatch_with_arg(id, ctx, Some("25"))` on the SAME
    /// parameterized entry enqueues `PluginCall { id, arg: Some("25") }` DIRECTLY and opens NO
    /// minibuffer — an already-supplied arg must never be re-prompted.
    #[test]
    fn param_command_with_supplied_arg_does_not_reprompt() {
        let mut reg = Registry::builtins();
        let id = CommandId(crate::plugin::intern("param-cmd-test.supplied-arg"));
        reg.register_plugin(id, "Param Cmd", None, Some("Minutes:"))
            .expect("register_plugin should succeed on a fresh id");

        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch_with_arg(id, &mut ctx, Some("25".into()));

        assert_eq!(r, CommandResult::Handled);
        assert!(e.minibuffer.is_none(), "a supplied arg must never open the prompt");
        assert_eq!(e.pending_plugin_calls.len(), 1);
        assert_eq!(e.pending_plugin_calls[0], crate::plugin::PluginCall { id, arg: Some("25".into()) });
    }

    /// Case 4 — a `Plugin` entry with `meta.arg == None`, dispatched via `dispatch` (no arg):
    /// pushes `PluginCall { id, arg: None }`, no minibuffer.
    #[test]
    fn nullary_plugin_command_dispatches_with_none() {
        let mut reg = Registry::builtins();
        let id = CommandId(crate::plugin::intern("param-cmd-test.nullary"));
        reg.register_plugin(id, "Nullary Cmd", None, None)
            .expect("register_plugin should succeed on a fresh id");

        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch(id, &mut ctx);

        assert_eq!(r, CommandResult::Handled);
        assert!(e.minibuffer.is_none());
        assert_eq!(e.pending_plugin_calls.len(), 1);
        assert_eq!(e.pending_plugin_calls[0], crate::plugin::PluginCall { id, arg: None });
    }

    /// Case 1 — a builtin dispatched with a supplied arg runs the builtin and drops the arg
    /// (builtins take no arg today).
    #[test]
    fn builtin_dispatch_ignores_supplied_arg() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch_with_arg(CommandId("save"), &mut ctx, Some("x".into()));
        assert_eq!(r, CommandResult::Handled);
    }

    #[test]
    fn retain_builtins_keeps_builtins_and_drops_plugins() {
        let mut reg = Registry::builtins();
        let builtin_count = reg.commands().count();
        let id_a = CommandId(crate::plugin::intern("retain-test.a"));
        let id_b = CommandId(crate::plugin::intern("retain-test.b"));
        reg.register_plugin(id_a, "A", None, None).expect("register_plugin should succeed on a fresh id");
        reg.register_plugin(id_b, "B", None, None).expect("register_plugin should succeed on a fresh id");

        reg.retain_builtins();

        assert_eq!(reg.resolve_name("retain-test.a"), None);
        assert_eq!(reg.resolve_name("retain-test.b"), None);
        assert_eq!(reg.resolve_name("save"), Some(CommandId("save")));
        assert_eq!(reg.commands().count(), builtin_count);

        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch(CommandId("save"), &mut ctx);
        assert_eq!(r, crate::commands::CommandResult::Handled);
    }

    #[test]
    fn retain_builtins_reindexes_so_a_reregister_succeeds() {
        let mut reg = Registry::builtins();
        let id = CommandId(crate::plugin::intern("retain-test.reregister"));
        reg.register_plugin(id, "Once", None, None).expect("register_plugin should succeed on a fresh id");
        reg.retain_builtins();

        reg.register_plugin(id, "Again", None, None)
            .expect("re-registering the same id after retain_builtins should succeed — no ghost index entry");
        assert_eq!(reg.resolve_name("retain-test.reregister"), Some(id));
        assert_eq!(reg.meta(id).unwrap().label, "Again");

        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch(id, &mut ctx);
        assert_eq!(r, CommandResult::Handled);
        assert_eq!(e.pending_plugin_calls.len(), 1, "re-registered id dispatches as a plugin call");
        assert_eq!(e.pending_plugin_calls[0], crate::plugin::PluginCall { id, arg: None });
    }

    #[test]
    fn retain_builtins_preserves_builtin_order() {
        let before: Vec<&str> = Registry::builtins().commands().map(|(id, _)| id.0).collect();

        let mut reg = Registry::builtins();
        let id = CommandId(crate::plugin::intern("retain-test.order"));
        reg.register_plugin(id, "Order", None, None).expect("register_plugin should succeed on a fresh id");
        reg.retain_builtins();
        let after: Vec<&str> = reg.commands().map(|(id, _)| id.0).collect();

        assert_eq!(before, after, "builtin palette order must be stable across a plugin register+retain cycle");
    }

    #[test]
    fn unknown_command_surfaces_status_not_silent() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch(CommandId("nope"), &mut ctx);
        assert_eq!(r, crate::commands::CommandResult::Noop);
        assert!(e.status_text().contains("unknown command"), "must surface, never silent (§12.5)");
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

    /// Task 4.2: switch_buffer/next_buffer/prev_buffer are registered + palette-listed
    /// but menu-absent — the Documents dynamic menu supersedes them on the menu surface.
    #[test]
    fn switch_buffer_is_registered_in_view_menu() {
        let reg = Registry::builtins();
        let m = reg.meta(CommandId("switch_buffer"))
            .expect("switch_buffer must be registered");
        assert_eq!(m.label, "Switch Buffer\u{2026}");
        assert_eq!(m.menu, None, "switch_buffer is palette-only — Documents supersedes it on the menu");
        for id in ["next_buffer", "prev_buffer"] {
            let m = reg.meta(CommandId(id)).unwrap_or_else(|| panic!("{id} must be registered"));
            assert_eq!(m.menu, None, "{id} is palette-only — Documents supersedes it on the menu");
        }
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
    // Task 3.2 (A3b): menu-placement sweep — filter/transform → Format,
    // keystroke-native deletes → palette-only.
    // -----------------------------------------------------------------------

    #[test]
    fn a3b_placement_sweep_categories() {
        let reg = Registry::builtins();
        let meta = |id: &str| reg.meta(reg.resolve_name(id).expect(id)).expect(id);
        assert_eq!(meta("filter").menu, Some(MenuCategory::Format),
            "filter is a text-shaping op, sibling of reflow/unwrap/ventilate");
        assert_eq!(meta("transform").menu, None,
            "A16: the Transform… umbrella row is dropped from the Format menu — its discrete \
             variants (reflow/unwrap/ventilate) already carry the Format door; the command stays \
             registered and palette-reachable");
        for id in ["delete_word_back", "delete_word_forward", "delete_line", "delete_to_line_end"] {
            assert_eq!(meta(id).menu, None, "{id} is a keystroke-native atomic edit — palette-only");
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
            ("set_wrap_column",  "Wrap Column: Set\u{2026}"),
            ("toggle_chrome",    "Chrome: Full/Zen"),
            ("save_settings",    "Save Settings"),
            ("plugins_reload",   "Reload Plugins"),
            ("plugin_list",      "List Plugins"),
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
    // P2 Task 8b: plugins_reload / plugin_list builtins (§6)
    // -----------------------------------------------------------------------

    #[test]
    fn plugins_reload_sets_flag() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        assert!(!e.plugins_reload_requested, "flag starts clear");
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch(CommandId("plugins_reload"), &mut ctx);
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert!(e.plugins_reload_requested, "plugins_reload sets the request flag the seam consumes");
    }

    #[test]
    fn plugin_list_formats_inventory() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.plugin_inventory = vec![
            crate::plugin::PluginRecord { name: "a".into(), commands: 2, hooks: 1, error: None },
            crate::plugin::PluginRecord { name: "b".into(), commands: 0, hooks: 0, error: Some("boom".into()) },
        ];
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch(CommandId("plugin_list"), &mut ctx);
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert_eq!(e.status_text(), "plugins: 1 ok (2 cmds, 1 hooks, 0 timers), 1 failed");
    }

    #[test]
    fn plugin_list_reports_armed_timers() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.plugin_inventory = vec![crate::plugin::PluginRecord {
            name: "a".into(),
            commands: 2,
            hooks: 1,
            error: None,
        }];
        for handle in 0..2 {
            e.pending_plugin_timers.push(crate::plugin::PluginTimer {
                handle,
                origin: "a".into(),
                key: format!("wc-timer-{handle}"),
                next_due_ms: 1_000,
                interval_ms: 1_000,
                repeat: false,
                pending: false,
            });
        }
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        let r = reg.dispatch(CommandId("plugin_list"), &mut ctx);
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert_eq!(e.status_text(), "plugins: 1 ok (2 cmds, 1 hooks, 2 timers), 0 failed");
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
        assert_eq!(ed.status_text(), "canvas: transparent");
        // Non-Rgb theme: flips + persists, honest "no effect".
        let mut ed2 = crate::editor::Editor::new_from_text("x", None, (40, 4));
        ed2.theme = wordcartel_core::theme::Theme::builtin("terminal-plain").unwrap();
        ed2.depth = Depth::Truecolor;
        toggle_canvas(&mut ed2);
        assert_eq!(ed2.canvas, CanvasMode::Transparent, "flip persists even when inert");
        assert_eq!(ed2.status_text(), "canvas: transparent (no effect: terminal-plain has no canvas)");
        // Depth::None (cue) on an Rgb theme: also "no effect" (no color to paint).
        let mut ed3 = crate::editor::Editor::new_from_text("x", None, (40, 4));
        ed3.theme = wordcartel_core::theme::Theme::builtin("flexoki-dark").unwrap();
        ed3.depth = Depth::None;
        toggle_canvas(&mut ed3);
        assert_eq!(ed3.canvas, CanvasMode::Transparent);
        assert_eq!(ed3.status_text(), "canvas: transparent (no effect: flexoki-dark has no canvas)");
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
        assert!(ed.status_text().contains("chrome: zen"), "status must say 'chrome: zen': {:?}", ed.status_text());
        // Second toggle flips back to Full.
        dispatch_id(&mut ed, "toggle_chrome");
        assert_eq!(ed.chrome_disposition, ChromeDisposition::Full, "second toggle → Full");
        assert!(ed.status_text().contains("chrome: full"), "status: {:?}", ed.status_text());
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
        assert_eq!(ed.status_text(), "chrome: n/a (cue mode)", "cue mode status: {:?}", ed.status_text());
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
        assert!(ed.status_text().contains("no effect:"), "must warn 'no effect': {:?}", ed.status_text());
        assert!(ed.status_text().contains("has fixed chrome"), "must say 'has fixed chrome': {:?}", ed.status_text());
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
        assert!(ed.status_text().contains("no effect at 16-color depth"),
            "must warn 16-color: {:?}", ed.status_text());
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
        ed.active_mut().view.mode = crate::editor::RenderMode::Review; // §2.5: diag_next is Review-only
        let a_byte = doc.find("## A").unwrap();
        let bad_byte = doc.find("bad_word").unwrap();

        // Seed the DiagStore with a diagnostic inside ## A's body.
        let v = ed.active().document.version;
        ed.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).diagnostics = vec![
            wordcartel_core::diagnostics::Diagnostic {
                range: bad_byte..(bad_byte + "bad_word".len()),
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                message: "x".into(),
                suggestions: vec![],
            }
        ];
        ed.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).computed_version = v;

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
        ed.active_mut().view.mode = crate::editor::RenderMode::Review; // §2.5: diag_prev is Review-only
        let a_byte = doc.find("## A").unwrap();
        let bad_byte = doc.find("bad_word").unwrap();

        // Seed the DiagStore with a diagnostic inside ## A's body.
        let v = ed.active().document.version;
        ed.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).diagnostics = vec![
            wordcartel_core::diagnostics::Diagnostic {
                range: bad_byte..(bad_byte + "bad_word".len()),
                kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                message: "x".into(),
                suggestions: vec![],
            }
        ];
        ed.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).computed_version = v;

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
    // Task 4 (Effort 7): render-mode command surface — set_render_mode, view_*
    // primitives, stateful cycle, Review-only diag guards (spec §2.5, §3.1-3.5)
    // -----------------------------------------------------------------------

    #[test]
    fn view_review_command_enters_review_and_arms() {
        use crate::editor::RenderMode;
        let mut ed = Editor::new_from_text("x\n", None, (80, 24));
        // arm_enabled (Task 3) only arms slots of INSTALLED+enabled sources — install Harper so
        // the arm-on-enter-Review behavior below has something to arm.
        ed.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(wordcartel_core::diagnostics::DiagSource::Harper)), true);
        ed.diag_cfg.enabled = true;
        assert_eq!(ed.active().view.mode, RenderMode::LivePreview);
        dispatch_id(&mut ed, "view_review");
        assert_eq!(ed.active().view.mode, RenderMode::Review);
        assert_eq!(ed.active().diagnostics.slot(wordcartel_core::diagnostics::DiagSource::Harper)
            .unwrap().recheck_due_at, Some(0), "arm-on-enter at debounce 0 (Z clock now=0)");
    }

    #[test]
    fn cycle_render_mode_state_label_tracks_mode() {
        use crate::editor::RenderMode;
        let reg = Registry::builtins();
        let m = reg.meta(CommandId("cycle_render_mode")).unwrap();
        assert_eq!(m.menu, Some(MenuCategory::View));
        let state = m.state.expect("cycle_render_mode must be stateful");
        let mut ed = Editor::new_from_text("x\n", None, (80, 24));
        for (mode, expected) in [
            (RenderMode::LivePreview, "Live"),
            (RenderMode::Review, "Review"),
            (RenderMode::SourceHighlighted, "SRC-HI"),
            (RenderMode::SourcePlain, "Source"),
        ] {
            ed.active_mut().view.mode = mode;
            assert_eq!(state(&ed), MenuMark::Value(expected), "label for {mode:?}");
        }
    }

    #[test]
    fn diag_actions_are_review_only() {
        // Seed a valid diagnostic under the caret; try quick_fix/diag_next/diag_prev
        // in LivePreview (must no-op) and then in Review (must act).
        let doc = "teh cat\n";
        let seed = |ed: &mut Editor| {
            let v = ed.active().document.version;
            ed.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
                range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
                source: wordcartel_core::diagnostics::DiagSource::Harper, code: None, href: None,
                message: "x".into(),
                suggestions: vec![] }];
            ed.active_mut().diagnostics.slot_mut(wordcartel_core::diagnostics::DiagSource::Harper).computed_version = v;
            ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        };

        // quick_fix: LivePreview no-ops with the "no diagnostic here" status and no overlay.
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        seed(&mut ed);
        dispatch_id(&mut ed, "quick_fix");
        assert!(ed.diag.is_none(), "quick_fix must not open the overlay outside Review");
        assert_eq!(ed.status_text(), "no diagnostic here");

        // quick_fix: Review opens the overlay.
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        seed(&mut ed);
        ed.active_mut().view.mode = crate::editor::RenderMode::Review;
        dispatch_id(&mut ed, "quick_fix");
        assert!(ed.diag.is_some(), "quick_fix must open the overlay in Review");

        // diag_next: LivePreview leaves the selection unchanged.
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        seed(&mut ed);
        let before = ed.active().document.selection.clone();
        dispatch_id(&mut ed, "diag_next");
        assert_eq!(ed.active().document.selection, before, "diag_next must no-op outside Review");

        // diag_next: Review moves the caret.
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        seed(&mut ed);
        ed.active_mut().view.mode = crate::editor::RenderMode::Review;
        dispatch_id(&mut ed, "diag_next");
        assert_eq!(ed.active().document.selection.primary().head, 0, "diag_next must land on the diagnostic in Review");

        // diag_prev: LivePreview leaves the selection unchanged.
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        seed(&mut ed);
        let before = ed.active().document.selection.clone();
        dispatch_id(&mut ed, "diag_prev");
        assert_eq!(ed.active().document.selection, before, "diag_prev must no-op outside Review");

        // diag_prev: Review moves the caret.
        let mut ed = Editor::new_from_text(doc, None, (80, 24));
        seed(&mut ed);
        ed.active_mut().view.mode = crate::editor::RenderMode::Review;
        dispatch_id(&mut ed, "diag_prev");
        assert_eq!(ed.active().document.selection.primary().head, 0, "diag_prev must land on the diagnostic in Review");
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
    fn set_wrap_column_is_stateful_with_value_label() {
        let reg = Registry::builtins();
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
        let meta = reg.meta(CommandId("set_wrap_column")).unwrap();
        assert!(meta.state.is_some(), "set_wrap_column must be stateful");
        ed.view_opts.wrap_column = 80;
        assert_eq!((meta.state.unwrap())(&ed), MenuMark::Text("80\u{2026}".into()));
    }

    #[test]
    fn keymap_group_collapses_to_one_cycle_row() {
        let reg = Registry::builtins();
        // keymap_cua/keymap_wordstar are palette-only now (menu: None).
        assert_eq!(reg.meta(CommandId("keymap_cua")).unwrap().menu, None);
        assert_eq!(reg.meta(CommandId("keymap_next")).unwrap().menu, Some(MenuCategory::Settings));
    }

    // -----------------------------------------------------------------------
    // Task 7: analysis-lens/enable commands — registration + dispatch.
    // -----------------------------------------------------------------------

    #[test]
    fn analysis_commands_registered_and_dispatch() {
        let reg = Registry::builtins();
        assert!(reg.meta(CommandId("analysis_engine_harper")).is_some());
        assert!(reg.meta(CommandId("analysis_next")).is_some());
        assert!(reg.meta(CommandId("toggle_engine_harper")).is_some());
        assert_eq!(reg.meta(CommandId("analysis_engine_harper")).unwrap().menu, None,
            "set-primitive is palette-only");
        assert!(reg.meta(CommandId("analysis_next")).unwrap().menu.is_some(),
            "cycle carried in the menu");
        assert_eq!(reg.meta(CommandId("toggle_engine_harper")).unwrap().menu, None,
            "enablement toggle is palette-only");
    }

    #[test]
    fn analysis_next_dispatches_cycle_and_reports_state() {
        let mut ed = Editor::new_from_text("x\n", None, (40, 8));
        ed.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new()
                .with_source(wordcartel_core::diagnostics::DiagSource::Harper)), true);
        ed.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new()
                .with_source(wordcartel_core::diagnostics::DiagSource::Plugin("mock"))), true);
        let reg = Registry::builtins();
        let meta = reg.meta(CommandId("analysis_next")).unwrap();
        assert_eq!((meta.state.unwrap())(&ed), MenuMark::Value("Harper"));
        dispatch_id(&mut ed, "analysis_next");
        assert_eq!(ed.active_analysis_source, wordcartel_core::diagnostics::DiagSource::Plugin("mock"));
        assert_eq!((meta.state.unwrap())(&ed), MenuMark::Value("mock"));
    }

    #[test]
    fn toggle_engine_harper_dispatches_set_engine_enabled() {
        let mut ed = Editor::new_from_text("x\n", None, (40, 8));
        ed.diag_providers.install(Box::new(
            crate::diag_provider::RecordingProvider::new()
                .with_source(wordcartel_core::diagnostics::DiagSource::Harper)), false);
        assert!(!ed.diag_providers.is_enabled(wordcartel_core::diagnostics::DiagSource::Harper));
        dispatch_id(&mut ed, "toggle_engine_harper");
        assert!(ed.diag_providers.is_enabled(wordcartel_core::diagnostics::DiagSource::Harper));
        dispatch_id(&mut ed, "toggle_engine_harper");
        assert!(!ed.diag_providers.is_enabled(wordcartel_core::diagnostics::DiagSource::Harper));
    }

    // Helper: build a Ctx and dispatch a command id against the given Editor.
    fn dispatch_id(ed: &mut Editor, id: &'static str) {
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: ed, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        reg.dispatch(CommandId(id), &mut ctx);
    }

    // A17 T8 — the EPILOGUE residual: `block_move` must NOT clear `marked_block` on a read-only
    // buffer (content + status are already covered by categories (a)/(b) + the delegators; this
    // forces `block_move` into the `register_mut` set via the dispatch guard).
    #[test]
    fn block_move_on_read_only_leaves_marked_block_and_content_intact() {
        let mut e = Editor::new_from_text("one two three\n", None, (40, 6));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 3, hidden: false });
        e.active_mut().read_only = true;
        let before = e.active().document.buffer.to_string();
        let mark_before = e.active().marked_block;
        dispatch_id(&mut e, "block_move"); // the dispatch mutates-guard fires before the handler
        assert_eq!(e.active().document.buffer.to_string(), before, "read-only: no content change");
        assert_eq!(e.active().marked_block, mark_before, "read-only: marked_block NOT cleared (no epilogue)");
        assert_eq!(e.status_text(), "buffer is read-only");
        assert_ne!(e.status_text(), "block moved", "must NOT report false success");
    }

    // A17 T8 — MECHANICAL completeness sweep: the source of truth for the `register_mut` set.
    // Content-unchanged is universal by categories (a)+(b); the load-bearing assertion is
    // `marked_block`-unchanged, which fails for any registry handler that runs a mutating epilogue
    // on the read-only buffer — forcing its `register_mut` (or an entry guard) until green.
    //
    // We track the read-only buffer BY ID, not via `e.active()`: a command that legitimately
    // SWITCHES the active buffer (`new`/`goto_scratch`/`next_buffer`/…) leaves the read-only
    // buffer's own content+mark intact and is NOT a mutation of it — only a mutating epilogue ON
    // this buffer clears its `marked_block` and fails. (Checking `e.active()` would conflate a
    // benign buffer-switch with a mutation and wrongly demand marking every switch command
    // `mutates`, trapping the user in the read-only view.) The two branches below split on whether
    // the view's id SURVIVES the command: if it survives, content+mark must be unchanged; if it is
    // GONE (a `close_buffer` dispose), the sanctioned reset must have discarded the content to a
    // fresh writable buffer — no writable slot may carry the read-only content forward.
    #[test]
    fn no_registry_command_runs_a_mutating_epilogue_on_a_read_only_buffer() {
        let reg = Registry::builtins();
        let ids: Vec<_> = reg.commands().map(|(id, _)| id).collect();
        let mut violations = Vec::new();
        for id in ids {
            // `view_messages` IS the regeneration seam for the read-only view — the one sanctioned
            // content-replacer (a regenerable projection of the ring, never user data; the same
            // principled exclusion as buffer disposal). It must run to regenerate, so it is not a
            // `mutates` command; exclude it from the sweep.
            if id.0 == "view_messages" { continue; }
            let mut e = Editor::new_from_text("one two three\n", None, (40, 6));
            e.install_scratch(); // so scratch-move epilogues (which early-return without a scratch buffer) execute
            // A realistic pre-edit state that exercises the mutating paths: a NON-EMPTY selection
            // "one" [0,3) (head 3, so `swap` and selection-driven surgery run) and a marked block
            // "three" [8,13) that does NOT contain the caret (so `block_move`'s move path — not its
            // "can't move into itself" early-return — executes). Both are non-overlapping.
            e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 3);
            e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 8, end: 13, hidden: false });
            e.active_mut().read_only = true;
            let view_id = e.active().id;
            let (content, mark) = (e.active().document.buffer.to_string(), e.active().marked_block);
            dispatch_id(&mut e, id.0);
            match e.buffers.iter().find(|b| b.id == view_id) {
                Some(b) => {
                    // The read-only view still exists: neither its content nor its `marked_block`
                    // may have changed (categories (a)/(b) + no mutating epilogue).
                    if b.document.buffer.to_string() != content { violations.push(format!("{} mutated content", id.0)); }
                    if b.marked_block != mark { violations.push(format!("{} ran a mutating epilogue (marked_block)", id.0)); }
                }
                None => {
                    // The command DISPOSED the read-only view (changed its BufferId — the
                    // last-ordinary reset in `workspace::close_buffer_now` replaces the slot with a
                    // FRESH untitled). A dispose is a distinct, SANCTIONED operation (not a content
                    // mutation): it must discard the read-only content to a fresh writable buffer,
                    // never smuggle that content INTO a now-writable slot. Assert no writable buffer
                    // now holds the snapshot content — this is what makes the sweep a genuine
                    // completeness proof that ALSO covers the dispose path (pre-fix a dispose passed
                    // silently because the `if let Some` skipped when `view_id` was gone).
                    if e.buffers.iter().any(|b| !b.read_only && b.document.buffer.to_string() == content) {
                        violations.push(format!(
                            "{} disposed the read-only view but a writable buffer now holds its content", id.0));
                    }
                }
            }
        }
        assert!(violations.is_empty(), "read-only guard incomplete: {violations:?}");
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

    // -----------------------------------------------------------------------
    // C1 Task 4: caret shape/blink option-reachability commands
    // -----------------------------------------------------------------------

    #[test]
    fn caret_shape_commands_set_and_cycle() {
        use crate::config::CaretShape;
        let mut ed = Editor::new_from_text("x\n", None, (80, 24));
        dispatch_id(&mut ed, "caret_shape_block");     assert_eq!(ed.caret_shape, CaretShape::Block);
        dispatch_id(&mut ed, "caret_shape_beam");      assert_eq!(ed.caret_shape, CaretShape::Beam);
        dispatch_id(&mut ed, "caret_shape_underline"); assert_eq!(ed.caret_shape, CaretShape::Underline);
        dispatch_id(&mut ed, "caret_shape_default");   assert_eq!(ed.caret_shape, CaretShape::Default);
        // cycle Default→Block→Beam→Underline→Default
        dispatch_id(&mut ed, "cycle_caret_shape"); assert_eq!(ed.caret_shape, CaretShape::Block);
        dispatch_id(&mut ed, "cycle_caret_shape"); assert_eq!(ed.caret_shape, CaretShape::Beam);
        dispatch_id(&mut ed, "cycle_caret_shape"); assert_eq!(ed.caret_shape, CaretShape::Underline);
        dispatch_id(&mut ed, "cycle_caret_shape"); assert_eq!(ed.caret_shape, CaretShape::Default);
        let reg = Registry::builtins();
        assert_eq!(reg.meta(CommandId("caret_shape_block")).unwrap().menu, None);
        assert_eq!(reg.meta(CommandId("cycle_caret_shape")).unwrap().menu, Some(MenuCategory::View));
    }

    #[test]
    fn caret_blink_commands_set_and_toggle() {
        let mut ed = Editor::new_from_text("x\n", None, (80, 24));
        dispatch_id(&mut ed, "caret_blink_off"); assert!(!ed.caret_blink);
        dispatch_id(&mut ed, "caret_blink_on");  assert!(ed.caret_blink);
        dispatch_id(&mut ed, "toggle_caret_blink"); assert!(!ed.caret_blink);
        dispatch_id(&mut ed, "toggle_caret_blink"); assert!(ed.caret_blink);
    }

    #[test]
    fn cursor_command_opens_picker() {
        let mut ed = Editor::new_from_text("x\n", None, (40, 12));
        dispatch_id(&mut ed, "cursor");
        assert!(ed.cursor_picker.is_some(), "the `cursor` command opens the picker");
    }

    #[test]
    fn splash_commands_move_view_opts_through_set_splash() {
        let reg = Registry::builtins();
        let mut e = Editor::new_from_text("hi\n", None, (80, 24));
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut e, clock: &clk, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        assert!(ctx.editor.view_opts.splash, "default on");
        let r = reg.dispatch(CommandId("splash_off"), &mut ctx);
        assert_eq!(r, crate::commands::CommandResult::Handled);
        assert!(!ctx.editor.view_opts.splash);
        assert!(ctx.editor.status_text().contains("next launch"),
            "status notes the deferred effect: {}", ctx.editor.status_text());
        reg.dispatch(CommandId("toggle_splash"), &mut ctx);
        assert!(ctx.editor.view_opts.splash, "toggle flips back on");
        reg.dispatch(CommandId("splash_on"), &mut ctx);
        assert!(ctx.editor.view_opts.splash, "absolute set is idempotent");
        reg.dispatch(CommandId("toggle_splash"), &mut ctx);
        assert!(!ctx.editor.view_opts.splash, "toggle flips off");
    }

    /// E7 T2: the "recheck_diagnostics" command arms a recheck only when the active buffer is in
    /// Review (draft-quiet) — outside Review it is a no-op (spec §2.2 item 3).
    #[test]
    fn recheck_diagnostics_arms_only_in_review() {
        use crate::editor::RenderMode;
        let mut ed = Editor::new_from_text("teh cat\n", None, (80, 24));
        // arm_enabled (Task 3) only arms slots of INSTALLED+enabled sources.
        ed.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(wordcartel_core::diagnostics::DiagSource::Harper)), true);
        ed.diag_cfg.enabled = true;

        ed.active_mut().view.mode = RenderMode::LivePreview;
        dispatch_id(&mut ed, "recheck_diagnostics");
        assert!(ed.active().diagnostics.slot(wordcartel_core::diagnostics::DiagSource::Harper).is_none(),
            "no-op outside Review");

        ed.active_mut().view.mode = RenderMode::Review;
        dispatch_id(&mut ed, "recheck_diagnostics");
        assert!(ed.active().diagnostics.slot(wordcartel_core::diagnostics::DiagSource::Harper)
            .unwrap().recheck_due_at.is_some(), "arms a recheck in Review");
    }

    /// S6 Task 7: `toggle_ventilate` is the stateful View-menu representative for the per-buffer
    /// `view.ventilate` lens (command-surface Law 2/6/8) — the state fn reads the ACTIVE buffer,
    /// and dispatch routes through the shared `ventilate::set_ventilate` setter.
    #[test]
    fn toggle_ventilate_is_stateful_onoff_and_flips_the_flag() {
        let reg = Registry::builtins();
        let mut ed = crate::editor::Editor::new_from_text("Hi there. Bye.\n", None, (40, 8));
        let m = reg.meta(CommandId("toggle_ventilate")).unwrap();
        assert_eq!(m.menu, Some(MenuCategory::View), "toggle_ventilate is a View row");
        let f = m.state.expect("toggle_ventilate is stateful");
        assert!(matches!(f(&ed), MenuMark::OnOff(false)), "defaults off");
        // Dispatch flips it on and rebuilds.
        let ex = InlineExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut ed, clock: &Z, executor: &ex, msg_tx: tx, fs: crate::test_support::test_fs() };
        assert_eq!(reg.dispatch(CommandId("toggle_ventilate"), &mut ctx), CommandResult::Handled);
        assert!(ed.active().view.ventilate, "dispatch turned the lens on");
        assert!(matches!(f(&ed), MenuMark::OnOff(true)));
    }

    // -----------------------------------------------------------------------
    // A17 T10 (Q6): messages_min_kind command-surface wiring — set-per-state
    // primitives + the 2-state toggle representative, all routed through the
    // single Editor::set_messages_min_kind setter.
    // -----------------------------------------------------------------------

    #[test]
    fn toggle_flips_between_two_states() {
        let mut e = Editor::new_from_text("x\n", None, (40, 6));
        e.set_messages_min_kind(crate::status::StatusKind::Info);
        dispatch_id(&mut e, "toggle_messages_verbosity");
        assert_eq!(e.messages_min_kind(), crate::status::StatusKind::Warning);
        dispatch_id(&mut e, "toggle_messages_verbosity");
        assert_eq!(e.messages_min_kind(), crate::status::StatusKind::Info);
    }

    #[test]
    fn set_per_state_primitives_set_the_floor_directly() {
        let mut e = Editor::new_from_text("x\n", None, (40, 6));
        dispatch_id(&mut e, "messages_min_warning");
        assert_eq!(e.messages_min_kind(), crate::status::StatusKind::Warning);
        dispatch_id(&mut e, "messages_min_info");
        assert_eq!(e.messages_min_kind(), crate::status::StatusKind::Info);
    }
}
