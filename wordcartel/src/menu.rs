use crate::keymap::KeyTrie;
use crate::registry::{CommandId, MenuCategory, Registry, MENU_ORDER};
use crate::app::Msg;
use crossterm::event::Event;

/// What a built menu row dispatches on activation (Enter / click). Exhaustive —
/// every activation site must place every variant (A8 seam; `SwitchBuffer` rows
/// are produced starting Task 4.2's Documents dynamic menu).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuRowAction {
    Command(crate::registry::CommandId),
    SwitchBuffer(crate::editor::BufferId),
}

#[derive(Clone, Debug)]
pub struct MenuView {
    pub groups: Vec<(MenuCategory, Vec<(String, MenuRowAction)>)>,
    pub open: usize,
    pub highlighted: usize,
    pub built: bool,
    pub scroll_top: usize,
}

pub fn empty() -> MenuView {
    MenuView { groups: Vec::new(), open: 0, highlighted: 0, built: false, scroll_top: 0 }
}

/// A placeholder opened AT a specific category (an index into `MENU_ORDER`);
/// hydration maps it to the built groups' position for that category.
pub fn empty_at(order_idx: usize) -> MenuView {
    MenuView { groups: Vec::new(), open: order_idx, highlighted: 0, built: false, scroll_top: 0 }
}

pub fn build(reg: &Registry, keymap: &KeyTrie, editor: &crate::editor::Editor) -> MenuView {
    MenuView { groups: grouped_commands(reg, keymap, editor), open: 0, highlighted: 0, built: true, scroll_top: 0 }
}

pub(crate) fn category_label_pub(cat: MenuCategory) -> &'static str {
    category_label(cat)
}

/// A live/data-driven menu section: rows are computed from `&Editor` state rather than
/// drawn from registered commands (contrast the static `raw` rows in `grouped_commands`,
/// below). The registration seam for such sections (module-structure GATE — new dynamic
/// sections are a ROW here, not inline bulk in `grouped_commands`).
pub struct DynamicSection {
    pub category: MenuCategory,
    pub rows: fn(&crate::editor::Editor) -> Vec<(String, MenuRowAction)>,
}

/// The Documents dynamic section (Task 4.2): one row per open buffer, data, not
/// registered commands — exempt from the palette/registry command surface.
pub const DYNAMIC_SECTIONS: &[DynamicSection] =
    &[DynamicSection { category: MenuCategory::Documents, rows: crate::workspace::documents_menu_rows }];

fn grouped_commands(reg: &Registry, keymap: &KeyTrie, editor: &crate::editor::Editor)
    -> Vec<(MenuCategory, Vec<(String, MenuRowAction)>)> {
    let mut groups = Vec::new();
    for cat in MENU_ORDER {
        // (base text, value, chord, action) intermediates — value and chord kept separate so the
        // group right-justifies into independent columns.
        let mut raw: Vec<(String, Option<String>, Option<String>, MenuRowAction)> = reg
            .commands()
            .filter_map(|(id, meta)| {
                if meta.menu == Some(cat) && id != CommandId("palette") {
                    let (base, value) = menu_leaf_parts(meta, editor);
                    Some((base, value, keymap.chord_for(id), MenuRowAction::Command(id)))
                } else {
                    None
                }
            })
            .collect();
        if cat == MenuCategory::View && reg.meta(CommandId("palette")).is_some() {
            raw.push(("Command Palette...".to_string(), None, keymap.chord_for(CommandId("palette")),
                MenuRowAction::Command(CommandId("palette"))));
        }
        for section in DYNAMIC_SECTIONS {
            if section.category == cat {
                raw.extend((section.rows)(editor).into_iter().map(|(label, action)| (label, None, None, action)));
            }
        }
        if !raw.is_empty() {
            groups.push((cat, right_justify_leaves(raw)));
        }
    }
    groups
}

/// Base menu leaf text and optional state value, split. Stateless → `(label, None)`.
/// Stateful → `(base, Some(value))` where `base` strips a leading "Toggle " and any ": variants".
fn menu_leaf_parts(meta: &crate::registry::CommandMeta, editor: &crate::editor::Editor)
    -> (String, Option<String>)
{
    use crate::registry::MenuMark;
    match meta.state {
        None => (meta.label.to_string(), None),
        Some(f) => {
            let base = meta.label.strip_prefix("Toggle ").unwrap_or(meta.label);
            let base = base.split(':').next().unwrap_or(base).trim().to_string();
            let value = match f(editor) {
                MenuMark::OnOff(b) => if b { "On" } else { "Off" }.to_string(),
                MenuMark::Value(v) => v.to_string(),
                MenuMark::Text(s) => s,
            };
            (base, Some(value))
        }
    }
}

/// Lay out a dropdown group into two independent right-aligned columns: an optional VALUE column
/// (stateful rows) and the CHORD column, matching the palette. `[base] … [value] … [chord]`.
/// A group with no stateful rows renders byte-identically to the chord-only layout. `GAP` is the
/// min gap before the chord; `VGAP` the gap between the base name and the value column.
fn right_justify_leaves<A>(raw: Vec<(String, Option<String>, Option<String>, A)>)
    -> Vec<(String, A)>
{
    const GAP: usize = 4;
    const VGAP: usize = 2;
    let cc = |s: &str| s.chars().count();
    let max_base = raw.iter().map(|(b, _, _, _)| cc(b)).max().unwrap_or(0);
    let max_val = raw.iter().filter_map(|(_, v, _, _)| v.as_deref().map(cc)).max().unwrap_or(0);
    let has_values = max_val > 0;
    // Left block: base name + (optional) right-aligned value column.
    let left_of = |base: &str, value: &Option<String>| -> String {
        if !has_values {
            base.to_string()
        } else {
            match value {
                Some(v) => format!("{:<mb$}{}{:>mv$}", base, " ".repeat(VGAP), v, mb = max_base, mv = max_val),
                None => format!("{:<w$}", base, w = max_base + VGAP + max_val),
            }
        }
    };
    // Chord column: right-justify to the widest (left-block + GAP + chord) over the group.
    let target = raw.iter()
        .map(|(b, v, c, _)| cc(&left_of(b, v)) + c.as_ref().map_or(0, |c| GAP + cc(c)))
        .max().unwrap_or(0);
    raw.into_iter()
        .map(|(base, value, chord, id)| {
            let label = match (&value, &chord) {
                (_, Some(c)) => {
                    let left = left_of(&base, &value);
                    let pad = target.saturating_sub(cc(&left) + cc(c));
                    format!("{left}{}{c}", " ".repeat(pad))
                }
                (Some(_), None) => left_of(&base, &value), // value column is last — no trailing pad
                (None, None) => base,                      // bare
            };
            (label, id)
        })
        .collect()
}

fn category_label(cat: MenuCategory) -> &'static str {
    match cat {
        MenuCategory::File => "File",
        MenuCategory::Edit => "Edit",
        MenuCategory::Block => "Block",
        MenuCategory::Format => "Format",
        MenuCategory::View => "View",
        MenuCategory::Documents => "Documents",
        MenuCategory::Settings => "Settings",
        MenuCategory::Export => "Export",
    }
}

/// Menu overlay intercepts KEY INPUT and PASTE (no text field; paste is
/// consumed / silently dropped). Non-key, non-paste messages fall through to
/// the normal handlers so background work continues while the menu is open.
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    reg: &crate::registry::Registry, keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled {
    if editor.menu.is_none() { return crate::app::Handled::Pass(msg); }
    if matches!(&msg, Msg::ClipboardPaste { .. }) {
        // Drop an async clipboard-paste result that arrives while the menu is
        // open — it must not land in the document behind the overlay.
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
    }
    if let Msg::Input(Event::Paste(_)) = &msg {
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
    }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            use crossterm::event::KeyCode;
            // Close OUTSIDE any menu borrow (Codex Critical: `editor.menu = None`
            // must not run while `editor.menu.as_mut()` is held).
            if matches!(k.code, KeyCode::Esc | KeyCode::F(10)) {
                editor.menu = None;
            } else {
                let mut selected: Option<MenuRowAction> = None;
                if let Some(menu) = editor.menu.as_mut() {   // borrow scoped to this block
                    let ncat = menu.groups.len();
                    match k.code {
                        KeyCode::Left if ncat > 0 => {
                            menu.open = (menu.open + ncat - 1) % ncat;
                            menu.highlighted = 0;
                            menu.scroll_top = 0; // reset window on category switch
                        }
                        KeyCode::Right if ncat > 0 => {
                            menu.open = (menu.open + 1) % ncat;
                            menu.highlighted = 0;
                            menu.scroll_top = 0; // reset window on category switch
                        }
                        KeyCode::Up if ncat > 0 => {
                            menu.highlighted = menu.highlighted.saturating_sub(1);
                            let n = menu.groups[menu.open].1.len();
                            // Coarse follow-the-selection layer — the paint re-windows against
                            // the true item-row budget every frame (list_window two-layer
                            // invariant), so this estimate need not reserve the indicator row.
                            let list_h = n.min(15);
                            crate::list_window::keep_visible(menu.highlighted, n, list_h, &mut menu.scroll_top);
                        }
                        KeyCode::Down if ncat > 0 => {
                            let n = menu.groups[menu.open].1.len();
                            if n > 0 {
                                menu.highlighted = (menu.highlighted + 1).min(n - 1);
                                // Coarse follow-the-selection layer — the paint re-windows against
                                // the true item-row budget every frame (list_window two-layer
                                // invariant), so this estimate need not reserve the indicator row.
                                let list_h = n.min(15);
                                crate::list_window::keep_visible(menu.highlighted, n, list_h, &mut menu.scroll_top);
                            }
                        }
                        KeyCode::Enter if ncat > 0 => {
                            if let Some((_, action)) = menu.groups[menu.open].1.get(menu.highlighted) { selected = Some(*action); }
                        }
                        _ => {}
                    }
                } // menu borrow dropped here
                if let Some(action) = selected {
                    dispatch_row_action(editor, reg, keymap, ex, clock, msg_tx, action);
                }
            }
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ex, clock, msg_tx));
    }
    // Non-key msg falls through to normal handling while menu stays open.
    crate::app::Handled::Pass(msg)
}

/// Dispatch a menu row's action on activation — shared by keyboard (`intercept`, above)
/// and mouse (`mouse::route_overlay`) so the two paths cannot drift. `Command(id)` runs
/// the registry command exactly as before this seam existed (`dispatch_overlay_command`
/// closes all overlays and hydrates any it opens). `SwitchBuffer(id)` jumps to the buffer
/// via the shared `workspace::switch_to` setter — the same Law-1/Law-6 compliant route the
/// palette's buffer-switcher rows already use (`mouse::route_overlay`'s palette branch).
/// The Documents dynamic menu (Task 4.2, `DYNAMIC_SECTIONS`) is the first built menu rows
/// to carry `SwitchBuffer`.
pub(crate) fn dispatch_row_action(
    editor: &mut crate::editor::Editor,
    reg: &crate::registry::Registry,
    keymap: &crate::keymap::KeyTrie,
    ex: &dyn crate::jobs::Executor,
    clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    action: MenuRowAction,
) {
    match action {
        MenuRowAction::Command(id) =>
            crate::app::dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id),
        MenuRowAction::SwitchBuffer(bid) => {
            editor.menu = None;
            if let Some(idx) = editor.buffers.iter().position(|b| b.id == bid) {
                crate::workspace::switch_to(editor, idx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn throwaway_editor() -> crate::editor::Editor {
        crate::editor::Editor::new_from_text("x\n", None, (80, 24))
    }

    fn top_level_labels(view: &MenuView) -> Vec<String> {
        let ed = throwaway_editor();
        grouped_commands(&crate::registry::Registry::builtins(), &test_keymap_for(view), &ed)
            .into_iter()
            .map(|(cat, _)| category_label(cat).to_string())
            .collect()
    }

    fn group_items(view: &MenuView, name: &str) -> Vec<(String, MenuRowAction)> {
        let ed = throwaway_editor();
        grouped_commands(&crate::registry::Registry::builtins(), &test_keymap_for(view), &ed)
            .into_iter()
            .find(|(cat, _)| category_label(*cat) == name)
            .map(|(_, items)| items)
            .unwrap_or_default()
    }

    fn test_keymap_for(_view: &MenuView) -> KeyTrie {
        let reg = crate::registry::Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        keymap
    }

    #[test]
    fn build_groups_by_category_in_order_with_chords_and_palette_entry() {
        let reg = crate::registry::Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ed = throwaway_editor();
        let view = build(&reg, &keymap, &ed);
        // top-level groups follow MENU_ORDER, only non-empty categories
        let tops = top_level_labels(&view); // helper: the group labels in order
        assert!(tops.contains(&"Edit".to_string()) && tops.contains(&"Format".to_string()));
        assert!(!tops.contains(&"Insert".to_string()), "empty/absent categories omitted");
        // a Format leaf carries its chord baked into the label (if bound) or just the label
        let fmt = group_items(&view, "Format"); // helper: leaf (label, MenuRowAction) pairs
        assert!(fmt.iter().any(|(label, action)|
            *action == MenuRowAction::Command(crate::registry::CommandId("reflow")) && label.starts_with("Reflow")));
        // View contains the palette cross-link
        let view_items = group_items(&view, "View");
        assert!(view_items.iter().any(|(_, action)|
            *action == MenuRowAction::Command(crate::registry::CommandId("palette"))));
    }

    /// A8 seam: after the `MenuRowAction` refactor, static rows still dispatch exactly as
    /// before — every built row is `Command(id)`, byte-identical behavior to pre-refactor.
    #[test]
    fn command_rows_still_dispatch_after_action_refactor() {
        let reg = crate::registry::Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ed = throwaway_editor();
        let view = build(&reg, &keymap, &ed);
        let fmt = group_items(&view, "Format");
        assert!(fmt.iter().any(|(_, action)|
            *action == MenuRowAction::Command(CommandId("reflow"))),
            "reflow row must carry MenuRowAction::Command(CommandId(\"reflow\")): {fmt:?}");
        let view_items = group_items(&view, "View");
        assert!(view_items.iter().any(|(_, action)|
            *action == MenuRowAction::Command(CommandId("palette"))),
            "palette cross-link row must carry MenuRowAction::Command(CommandId(\"palette\")): {view_items:?}");
    }

    #[test]
    fn menu_leaf_shows_state_in_label() {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
        ed.view_opts.word_count = true;
        let groups = grouped_commands(&reg, &km, &ed);
        let view = groups.iter().find(|(c, _)| *c == crate::registry::MenuCategory::View).unwrap();
        // A7: base name and value shown, value in its own column — the glued "Word Count: On" is gone.
        assert!(view.1.iter().any(|(label, _)|
            label.starts_with("Word Count") && label.contains("On") && !label.contains("Word Count: On")),
            "stateful toggle shows 'Word Count' + 'On' in a column, got {:?}", view.1);
    }

    #[test]
    fn wrap_column_row_shows_value() {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
        ed.view_opts.wrap_column = 80;
        let groups = grouped_commands(&reg, &km, &ed);
        let settings = groups.iter().find(|(c, _)| *c == crate::registry::MenuCategory::Settings).unwrap();
        assert!(settings.1.iter().any(|(label, _)|
            label.starts_with("Wrap Column") && label.contains("80") && label.contains('\u{2026}')),
            "wrap column row must show its live value: {:?}", settings.1);
    }

    // -----------------------------------------------------------------------
    // LAW 7 (command-surface contract): custom binding surfaces in menu + palette
    // -----------------------------------------------------------------------

    #[test]
    fn menu_chords_are_right_justified_within_a_group() {
        // 4-tuple: (base, value, chord, id). Chords still flush-right; values (when present) share a column.
        let raw = vec![
            ("Cut".to_string(), None, Some("ctrl-x".to_string()), CommandId("cut")),
            ("Copy As Something Long".to_string(), None, Some("ctrl-c".to_string()), CommandId("copy")),
            ("No Chord Item".to_string(), None, None, CommandId("noop")),
        ];
        let leaves = right_justify_leaves(raw);
        assert!(leaves[0].0.ends_with("ctrl-x"));
        assert!(leaves[1].0.ends_with("ctrl-c"));
        assert_eq!(leaves[0].0.chars().count(), leaves[1].0.chars().count());
        assert_eq!(leaves[2].0, "No Chord Item", "no value + no chord → bare base, no trailing pad");
    }

    #[test]
    fn menu_values_share_a_right_aligned_column() {
        // Differing base widths, values of differing widths → all value right-edges align. One row
        // carries BOTH a value and a chord (the user-customized-binding case).
        let raw = vec![
            ("Clipboard".to_string(), Some("Auto".to_string()), None, CommandId("a")),
            ("Keymap".to_string(), Some("CUA".to_string()), Some("ctrl-k".to_string()), CommandId("b")),
            ("Word Count".to_string(), Some("Off".to_string()), None, CommandId("c")),
            ("Plain Item".to_string(), None, None, CommandId("d")),
        ];
        let leaves = right_justify_leaves(raw);
        // The value-column right edge is common: the char index just past each value aligns.
        let val_end = |s: &str, v: &str| s.find(v).map(|i| i + v.chars().count());
        let e_clip = val_end(&leaves[0].0, "Auto").unwrap();
        let e_wc   = val_end(&leaves[2].0, "Off").unwrap();
        assert_eq!(e_clip, e_wc, "value right edges must align: {:?}", leaves);
        // The both-columns row keeps the chord flush-right after the value column.
        assert!(leaves[1].0.contains("CUA") && leaves[1].0.ends_with("ctrl-k"));
        // A plain row with neither is bare.
        assert_eq!(leaves[3].0, "Plain Item");
    }

    // -----------------------------------------------------------------------
    // Task 2.1 (command-surface curation, A10): Block category
    // -----------------------------------------------------------------------

    #[test]
    fn block_category_groups_the_block_family() {
        let reg = crate::registry::Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ed = throwaway_editor();
        let view = build(&reg, &keymap, &ed);
        let tops = top_level_labels(&view);
        let edit_pos = tops.iter().position(|l| l == "Edit").expect("Edit group must exist");
        let block_pos = tops.iter().position(|l| l == "Block").expect("Block group must exist");
        let format_pos = tops.iter().position(|l| l == "Format").expect("Format group must exist");
        assert!(block_pos > edit_pos && block_pos < format_pos,
            "Block must sit edit-adjacent, between Edit and Format: {tops:?}");
        let block_items = group_items(&view, "Block");
        for id in ["block_begin", "block_write", "copy_block_to_scratch", "select_marked_block"] {
            assert!(block_items.iter().any(|(_, action)| *action == MenuRowAction::Command(crate::registry::CommandId(id))),
                "Block group must contain {id}: {block_items:?}");
        }
        let file_items = group_items(&view, "File");
        assert!(!file_items.iter().any(|(_, action)| *action == MenuRowAction::Command(crate::registry::CommandId("block_write"))),
            "block_write must have moved out of File");
    }

    #[test]
    fn custom_bind_surfaces_in_menu_and_palette() {
        let reg = crate::registry::Registry::builtins();
        let patch = crate::config::KeymapPatch {
            bind: [("ctrl-alt-c".to_string(), "cut".to_string())].into_iter().collect(),
            unbind: vec![], cua: None, wordstar: None,
        };
        let (km, _) = crate::keymap::build_keymap(
            &crate::config::KeymapConfig { preset: "cua".into(), patches: vec![patch] }, &reg);
        let ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
        // Menu bakes chord_for into the leaf label — the user's explicit binding must win.
        let groups = build(&reg, &km, &ed).groups;
        assert!(groups.iter().any(|(_, ls)| ls.iter().any(|(label, action)|
            *action == MenuRowAction::Command(crate::registry::CommandId("cut")) && label.contains("ctrl-alt-c"))),
            "menu hint must contain 'ctrl-alt-c' for cut");
        // Palette row.chord must also reflect the user's explicit binding.
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &km);
        assert!(p.rows.iter().any(|r|
            r.id == crate::registry::CommandId("cut") && r.chord == "ctrl-alt-c"),
            "palette hint must be 'ctrl-alt-c' for cut");
    }

    // -----------------------------------------------------------------------
    // Effort P1 (spec §9): LAW 4 + LAW 7 over a plugin-loaded registry
    // -----------------------------------------------------------------------

    /// LAW 4 — a plugin command tagged `menu=Some(Edit)` is a registered command, hence
    /// appears in BOTH the Edit menu group and the palette; a `menu=None` sibling is
    /// palette-only. Plugin entries obey menu ⊆ palette exactly like builtins — no dynamic
    /// section, no parallel path.
    #[test]
    fn plugin_menu_tagged_command_appears_in_menu_menu_none_is_palette_only() {
        let mut reg = crate::registry::Registry::builtins();
        let host = crate::plugin::host::PluginHost::new().expect("VM construction");
        let src = "wc.register_command{ name='a', label='Plugin Edit Thing', menu='Edit', fn=function() end }\n\
                   wc.register_command{ name='b', label='Plugin Palette Only', fn=function() end }";
        let reports =
            crate::plugin::load::load_sources(&mut reg, &host, &[("p1menu".to_string(), src.to_string())]);
        assert_eq!(reports[0].result, Ok(2), "test plugin must load cleanly: {:?}", reports[0].result);
        let a_id = reg.resolve_name("p1menu.a").expect("registered");
        let b_id = reg.resolve_name("p1menu.b").expect("registered");

        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let ed = throwaway_editor();
        let view = build(&reg, &km, &ed);
        let edit_items: Vec<(String, MenuRowAction)> = view.groups.iter()
            .find(|(cat, _)| *cat == crate::registry::MenuCategory::Edit)
            .map(|(_, items)| items.clone())
            .unwrap_or_default();
        assert!(edit_items.iter().any(|(_, action)| *action == MenuRowAction::Command(a_id)),
            "menu=Some(Edit) plugin command must appear in the Edit menu group: {edit_items:?}");
        assert!(view.groups.iter().all(|(_, items)|
            !items.iter().any(|(_, action)| *action == MenuRowAction::Command(b_id))),
            "menu=None plugin command must appear in NO menu group");

        // Palette side: BOTH appear — the menu-tagged AND the palette-only command.
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &km);
        assert!(p.rows.iter().any(|r| r.id == a_id), "menu-tagged plugin command missing from palette");
        assert!(p.rows.iter().any(|r| r.id == b_id), "palette-only plugin command missing from palette");
    }

    /// LAW 7 — a `keymap.patches` binding of a plugin command resolves in `build_keymap`
    /// (via `reg.resolve_name`, the same mechanism a builtin binds through) and SURVIVES a
    /// CUA↔WordStar preset switch (the patch is re-applied against whichever preset base is
    /// active, so the plugin binding is preset-independent — the same guarantee
    /// `custom_bind_surfaces_in_menu_and_palette` proves for `cut` above, now proven for a
    /// plugin command).
    #[test]
    fn plugin_command_bound_via_patch_resolves_and_survives_preset_switch() {
        let mut reg = crate::registry::Registry::builtins();
        let host = crate::plugin::host::PluginHost::new().expect("VM construction");
        let src = "wc.register_command{ name='cmd', label='Plugin Bound', fn=function() end }";
        let reports =
            crate::plugin::load::load_sources(&mut reg, &host, &[("p1law7".to_string(), src.to_string())]);
        assert_eq!(reports[0].result, Ok(1), "test plugin must load cleanly: {:?}", reports[0].result);
        let id = reg.resolve_name("p1law7.cmd").expect("registered");

        let patch = crate::config::KeymapPatch {
            bind: [("ctrl-alt-p".to_string(), "p1law7.cmd".to_string())].into_iter().collect(),
            unbind: vec![], cua: None, wordstar: None,
        };
        let (km_cua, warns_cua) = crate::keymap::build_keymap(
            &crate::config::KeymapConfig { preset: "cua".into(), patches: vec![patch.clone()] }, &reg);
        assert!(warns_cua.is_empty(), "patch must resolve cleanly under CUA: {warns_cua:?}");
        assert_eq!(km_cua.chord_for(id).as_deref(), Some("ctrl-alt-p"));

        let (km_ws, warns_ws) = crate::keymap::build_keymap(
            &crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![patch] }, &reg);
        assert!(warns_ws.is_empty(), "patch must resolve cleanly under WordStar: {warns_ws:?}");
        assert_eq!(km_ws.chord_for(id).as_deref(), Some("ctrl-alt-p"),
            "the plugin binding must survive the CUA -> WordStar preset switch");
    }

    // -----------------------------------------------------------------------
    // Task 4.2 (command-surface curation, A8): Documents dynamic section
    // -----------------------------------------------------------------------

    struct TestClock(u64);
    impl wordcartel_core::history::Clock for TestClock {
        fn now_ms(&self) -> u64 { self.0 }
    }

    fn enter_key() -> crossterm::event::Event {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        crossterm::event::Event::Key(KeyEvent {
            code: KeyCode::Enter, modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press, state: KeyEventState::NONE,
        })
    }

    /// A8 / Task 4.2: the built menu has a Documents group whose rows are all
    /// `SwitchBuffer`, one per open (non-scratch) buffer; activating one via
    /// `intercept`'s Enter path switches the active buffer and closes the menu.
    #[test]
    fn documents_section_appears_and_switches() {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut ed = crate::editor::Editor::new_from_text("a\n", None, (40, 10));
        ed.install_scratch();
        let b_id = ed.alloc_id();
        let area = ed.active().view.area;
        ed.buffers.push(crate::editor::Buffer::from_text(b_id, "b\n", None, area));

        let view = build(&reg, &km, &ed);
        let docs_pos = view.groups.iter()
            .position(|(cat, _)| *cat == crate::registry::MenuCategory::Documents)
            .expect("Documents group must appear");
        let (docs_cat, docs_rows) = &view.groups[docs_pos];
        assert_eq!(*docs_cat, crate::registry::MenuCategory::Documents);
        assert_eq!(docs_rows.len(), 2, "two ordinary buffers, scratch excluded");
        assert!(docs_rows.iter().all(|(_, action)| matches!(action, MenuRowAction::SwitchBuffer(_))),
            "every Documents row must be SwitchBuffer: {docs_rows:?}");
        let b_row = docs_rows.iter().position(|(_, action)| *action == MenuRowAction::SwitchBuffer(b_id))
            .expect("Documents must contain a row for B");

        // Drive activation through the real intercept Enter path.
        ed.menu = Some(MenuView { groups: view.groups.clone(), open: docs_pos, highlighted: b_row,
            built: true, scroll_top: 0 });
        let ex = crate::jobs::InlineExecutor::default();
        let clk = TestClock(0);
        let (tx, _rx) = std::sync::mpsc::channel();
        let _ = intercept(crate::app::Msg::Input(enter_key()), &mut ed, &reg, &km, &ex, &clk, &tx);
        assert_eq!(ed.active().id, b_id, "selecting the Documents row switches to that buffer");
        assert!(ed.menu.is_none(), "menu closes after activation");
    }
}
