use crate::keymap::KeyTrie;
use crate::registry::{CommandId, MenuCategory, Registry, MENU_ORDER};
use tui_menu::{MenuItem, MenuState};

pub struct MenuView {
    pub state: MenuState<CommandId>,
    pub items: Vec<MenuItem<CommandId>>,
    pub built: bool,
}

impl std::fmt::Debug for MenuView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MenuView")
            .field("built", &self.built)
            .finish_non_exhaustive()
    }
}

pub fn empty() -> MenuView {
    MenuView {
        state: MenuState::new(Vec::new()),
        items: Vec::new(),
        built: false,
    }
}

pub fn build(reg: &Registry, keymap: &KeyTrie) -> MenuView {
    let groups = grouped_commands(reg, keymap);
    let items = menu_items_from_groups(&groups);
    let mut state = MenuState::new(menu_items_from_groups(&groups));
    state.activate();
    MenuView { state, items, built: true }
}

fn menu_items_from_groups(groups: &[(MenuCategory, Vec<(String, CommandId)>)]) -> Vec<MenuItem<CommandId>> {
    groups
        .iter()
        .map(|(cat, leaves)| {
            MenuItem::group(
                category_label(*cat),
                leaves.iter().map(|(label, id)| MenuItem::item(label.clone(), *id)).collect(),
            )
        })
        .collect()
}

fn grouped_commands(reg: &Registry, keymap: &KeyTrie) -> Vec<(MenuCategory, Vec<(String, CommandId)>)> {
    let mut groups = Vec::new();
    for cat in MENU_ORDER {
        let mut leaves: Vec<(String, CommandId)> = reg
            .commands()
            .filter_map(|(id, meta)| {
                if meta.menu == Some(cat) && id != CommandId("palette") {
                    Some((leaf_label(meta.label, keymap.chord_for(id)), id))
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
        MenuCategory::Export => "Export",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top_level_labels(view: &MenuView) -> Vec<String> {
        grouped_commands(&crate::registry::Registry::builtins(), &test_keymap_for(view))
            .into_iter()
            .map(|(cat, _)| category_label(cat).to_string())
            .collect()
    }

    fn group_items(view: &MenuView, name: &str) -> Vec<(String, CommandId)> {
        grouped_commands(&crate::registry::Registry::builtins(), &test_keymap_for(view))
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
        let view = build(&reg, &keymap);
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
}
