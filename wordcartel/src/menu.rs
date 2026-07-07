use crate::keymap::KeyTrie;
use crate::registry::{CommandId, MenuCategory, Registry, MENU_ORDER};

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
        // (base text, chord, id) intermediates — chord kept separate so the group right-justifies.
        let mut raw: Vec<(String, Option<String>, CommandId)> = reg
            .commands()
            .filter_map(|(id, meta)| {
                if meta.menu == Some(cat) && id != CommandId("palette") {
                    Some((menu_leaf_base(meta, editor), keymap.chord_for(id), id))
                } else {
                    None
                }
            })
            .collect();
        if cat == MenuCategory::View && reg.meta(CommandId("palette")).is_some() {
            raw.push(("Command Palette...".to_string(), keymap.chord_for(CommandId("palette")), CommandId("palette")));
        }
        if !raw.is_empty() {
            groups.push((cat, right_justify_leaves(raw)));
        }
    }
    groups
}

/// Base menu leaf text (state-in-label for stateful commands), WITHOUT the chord.
/// Stateless → static label. Stateful → `"{base}: {value}"` (base strips a leading
/// "Toggle " prefix and any "…: variants" suffix).
fn menu_leaf_base(meta: &crate::registry::CommandMeta, editor: &crate::editor::Editor) -> String {
    use crate::registry::MenuMark;
    match meta.state {
        None => meta.label.to_string(),
        Some(f) => {
            let base = meta.label.strip_prefix("Toggle ").unwrap_or(meta.label);
            let base = base.split(':').next().unwrap_or(base).trim();
            match f(editor) {
                MenuMark::OnOff(b) => format!("{base}: {}", if b { "On" } else { "Off" }),
                MenuMark::Value(v) => format!("{base}: {v}"),
            }
        }
    }
}

/// Right-justify chord hints within a dropdown group: pad each leaf so all chords end at a
/// common right column (flush-right), matching the command palette. Leaves with no chord render
/// as their base text. `GAP` is the minimum gap between the widest base label and its chord.
fn right_justify_leaves(raw: Vec<(String, Option<String>, CommandId)>) -> Vec<(String, CommandId)> {
    const GAP: usize = 4;
    let width = |base: &str, chord: &Option<String>| {
        base.chars().count() + chord.as_ref().map_or(0, |c| GAP + c.chars().count())
    };
    let target = raw.iter().map(|(b, c, _)| width(b, c)).max().unwrap_or(0);
    raw.into_iter()
        .map(|(base, chord, id)| {
            let label = match &chord {
                Some(c) => {
                    let pad = target.saturating_sub(base.chars().count() + c.chars().count());
                    format!("{base}{}{c}", " ".repeat(pad))
                }
                None => base,
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
        assert!(view.1.iter().any(|(label, _)| label.starts_with("Word Count: On")),
            "stateful toggle renders 'Word Count: On', got {:?}", view.1);
    }

    // -----------------------------------------------------------------------
    // LAW 7 (command-surface contract): custom binding surfaces in menu + palette
    // -----------------------------------------------------------------------

    #[test]
    fn menu_chords_are_right_justified_within_a_group() {
        // Leaves of differing label+chord widths → every chord ends at the same column (flush-right).
        let raw = vec![
            ("Cut".to_string(), Some("ctrl-x".to_string()), CommandId("cut")),
            ("Copy As Something Long".to_string(), Some("ctrl-c".to_string()), CommandId("copy")),
            ("No Chord Item".to_string(), None, CommandId("noop")),
        ];
        let leaves = right_justify_leaves(raw);
        assert!(leaves[0].0.ends_with("ctrl-x"));
        assert!(leaves[1].0.ends_with("ctrl-c"));
        // Chorded leaves share the target width, so their chords are flush-right at the same column.
        assert_eq!(leaves[0].0.chars().count(), leaves[1].0.chars().count());
        // A no-chord leaf renders as its base — no trailing padding/chord.
        assert_eq!(leaves[2].0, "No Chord Item");
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
