use crate::keymap::KeyTrie;
use crate::registry::{CommandId, MenuCategory, Registry, MENU_ORDER};
use crate::app::Msg;
use crossterm::event::Event;

#[derive(Clone, Debug)]
pub struct MenuView {
    pub groups: Vec<(MenuCategory, Vec<(String, CommandId)>)>,
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

fn grouped_commands(reg: &Registry, keymap: &KeyTrie, editor: &crate::editor::Editor)
    -> Vec<(MenuCategory, Vec<(String, CommandId)>)> {
    let mut groups = Vec::new();
    for cat in MENU_ORDER {
        // (base text, value, chord, id) intermediates — value and chord kept separate so the
        // group right-justifies into independent columns.
        let mut raw: Vec<(String, Option<String>, Option<String>, CommandId)> = reg
            .commands()
            .filter_map(|(id, meta)| {
                if meta.menu == Some(cat) && id != CommandId("palette") {
                    let (base, value) = menu_leaf_parts(meta, editor);
                    Some((base, value, keymap.chord_for(id), id))
                } else {
                    None
                }
            })
            .collect();
        if cat == MenuCategory::View && reg.meta(CommandId("palette")).is_some() {
            raw.push(("Command Palette...".to_string(), None, keymap.chord_for(CommandId("palette")), CommandId("palette")));
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
            };
            (base, Some(value))
        }
    }
}

/// Lay out a dropdown group into two independent right-aligned columns: an optional VALUE column
/// (stateful rows) and the CHORD column, matching the palette. `[base] … [value] … [chord]`.
/// A group with no stateful rows renders byte-identically to the chord-only layout. `GAP` is the
/// min gap before the chord; `VGAP` the gap between the base name and the value column.
fn right_justify_leaves(raw: Vec<(String, Option<String>, Option<String>, CommandId)>)
    -> Vec<(String, CommandId)>
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
        MenuCategory::Format => "Format",
        MenuCategory::View => "View",
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
        return crate::app::Handled::Done({ for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); } !editor.quit });
    }
    if let Msg::Input(Event::Paste(_)) = &msg {
        return crate::app::Handled::Done({ for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); } !editor.quit });
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
                            if let Some((_, id)) = menu.groups[menu.open].1.get(menu.highlighted) { selected = Some(*id); }
                        }
                        _ => {}
                    }
                } // menu borrow dropped here
                if let Some(id) = selected {
                    crate::app::dispatch_overlay_command(editor, reg, keymap, ex, clock, msg_tx, id);
                }
            }
        }
        return crate::app::Handled::Done({ for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); } !editor.quit });
    }
    // Non-key msg falls through to normal handling while menu stays open.
    crate::app::Handled::Pass(msg)
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

    fn group_items(view: &MenuView, name: &str) -> Vec<(String, CommandId)> {
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
        let fmt = group_items(&view, "Format"); // helper: leaf (label, CommandId) pairs
        assert!(fmt.iter().any(|(label, id)| *id == crate::registry::CommandId("reflow") && label.starts_with("Reflow")));
        // View contains the palette cross-link
        let view_items = group_items(&view, "View");
        assert!(view_items.iter().any(|(_, id)| *id == crate::registry::CommandId("palette")));
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
        assert!(groups.iter().any(|(_, ls)| ls.iter().any(|(label, id)|
            *id == crate::registry::CommandId("cut") && label.contains("ctrl-alt-c"))),
            "menu hint must contain 'ctrl-alt-c' for cut");
        // Palette row.chord must also reflect the user's explicit binding.
        let mut p = crate::palette::Palette::default();
        crate::palette::rebuild_rows(&mut p, &reg, &km);
        assert!(p.rows.iter().any(|r|
            r.id == crate::registry::CommandId("cut") && r.chord == "ctrl-alt-c"),
            "palette hint must be 'ctrl-alt-c' for cut");
    }
}
