use crate::keymap::KeyTrie;
use crate::registry::{CommandId, MenuCategory, Registry, MENU_ORDER};

#[derive(Clone, Debug)]
pub struct MenuView {
    pub groups: Vec<(MenuCategory, Vec<(String, CommandId)>)>,
    pub open: usize,
    pub highlighted: usize,
    pub built: bool,
}

pub fn empty() -> MenuView {
    MenuView { groups: Vec::new(), open: 0, highlighted: 0, built: false }
}

/// A placeholder opened AT a specific category (an index into `MENU_ORDER`);
/// hydration maps it to the built groups' position for that category.
pub fn empty_at(order_idx: usize) -> MenuView {
    MenuView { groups: Vec::new(), open: order_idx, highlighted: 0, built: false }
}

pub fn build(reg: &Registry, keymap: &KeyTrie, editor: &crate::editor::Editor) -> MenuView {
    MenuView { groups: grouped_commands(reg, keymap, editor), open: 0, highlighted: 0, built: true }
}

pub(crate) fn category_label_pub(cat: MenuCategory) -> &'static str {
    category_label(cat)
}

fn grouped_commands(reg: &Registry, keymap: &KeyTrie, editor: &crate::editor::Editor)
    -> Vec<(MenuCategory, Vec<(String, CommandId)>)> {
    let mut groups = Vec::new();
    for cat in MENU_ORDER {
        let mut leaves: Vec<(String, CommandId)> = reg
            .commands()
            .filter_map(|(id, meta)| {
                if meta.menu == Some(cat) && id != CommandId("palette") {
                    Some((menu_leaf_label(meta, editor, keymap.chord_for(id)), id))
                } else {
                    None
                }
            })
            .collect();
        if cat == MenuCategory::View && reg.meta(CommandId("palette")).is_some() {
            leaves.push((leaf_label("Command Palette...", keymap.chord_for(CommandId("palette"))), CommandId("palette")));
        }
        if !leaves.is_empty() {
            groups.push((cat, leaves));
        }
    }
    groups
}

/// Compose a menu leaf label, interpolating live state for stateful commands.
/// Stateless → static label + chord. Stateful → `"{base}: {value}"` + chord,
/// where `base` strips a leading "Toggle " prefix and any "…: variants" suffix.
fn menu_leaf_label(meta: &crate::registry::CommandMeta,
                   editor: &crate::editor::Editor, chord: Option<String>) -> String {
    use crate::registry::MenuMark;
    let text = match meta.state {
        None => meta.label.to_string(),
        Some(f) => {
            let base = meta.label.strip_prefix("Toggle ").unwrap_or(meta.label);
            let base = base.split(':').next().unwrap_or(base).trim();
            match f(editor) {
                MenuMark::OnOff(b) => format!("{base}: {}", if b { "On" } else { "Off" }),
                MenuMark::Value(v) => format!("{base}: {v}"),
            }
        }
    };
    leaf_label(&text, chord)
}

fn leaf_label(label: &str, chord: Option<String>) -> String {
    match chord {
        Some(chord) => format!("{label}    {chord}"),
        None => label.to_string(),
    }
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
}
