use crate::registry::{Registry, CommandId};
use crate::keymap::KeyTrie;
use nucleo_matcher::{Matcher, Config};
use nucleo_matcher::pattern::{Pattern, CaseMatching, Normalization};

#[derive(Default, Debug, Clone)]
pub struct Palette {
    pub query: String,
    pub cursor: usize,
    pub rows: Vec<PaletteRow>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct PaletteRow {
    pub id: CommandId,
    pub label: String,
    pub chord: String,
}

/// Rebuild the (precomputed) rows from the registry, ranked by `query`.
/// Empty query → all commands in registration order.
/// Non-empty query → matches only, sorted by score desc, ties broken by registration order (stable).
/// No match → empty rows. `selected` clamped.
pub fn rebuild_rows(p: &mut Palette, reg: &Registry, keymap: &KeyTrie) {
    let all: Vec<(CommandId, &str)> = reg.commands().map(|(id, m)| (id, m.label)).collect();
    let ranked: Vec<CommandId> = if p.query.is_empty() {
        all.iter().map(|(id, _)| *id).collect()
    } else {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pat = Pattern::parse(&p.query, CaseMatching::Ignore, Normalization::Smart);
        // score each label; keep matches; sort by score desc, then registration order (stable).
        let mut scored: Vec<(usize, u32, CommandId)> = all.iter().enumerate()
            .filter_map(|(i, (id, label))| {
                let mut buf = Vec::new();
                let hay = nucleo_matcher::Utf32Str::new(label, &mut buf);
                pat.score(hay, &mut matcher).map(|s| (i, s, *id))
            }).collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scored.into_iter().map(|(_, _, id)| id).collect()
    };
    p.rows = ranked.into_iter().map(|id| PaletteRow {
        id,
        label: reg.meta(id).map(|m| m.label.to_string()).unwrap_or_default(),
        chord: keymap.chord_for(id).unwrap_or_default(),
    }).collect();
    if p.selected >= p.rows.len() { p.selected = p.rows.len().saturating_sub(1); }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuild_rows_empty_query_lists_all_in_order_with_chords() {
        let reg = crate::registry::Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = Palette::default();
        rebuild_rows(&mut p, &reg, &keymap);
        assert_eq!(p.rows.len(), reg.commands().count(), "empty query → all commands");
        let cut = p.rows.iter().find(|r| r.id == crate::registry::CommandId("cut")).unwrap();
        assert_eq!(cut.label, "Cut");
        assert_eq!(cut.chord, "ctrl-x"); // its CUA chord
    }

    #[test]
    fn rebuild_rows_fuzzy_filters_and_ranks() {
        let reg = crate::registry::Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = Palette { query: "refl".into(), ..Default::default() };
        rebuild_rows(&mut p, &reg, &keymap);
        assert!(p.rows.iter().any(|r| r.id == crate::registry::CommandId("reflow")), "fuzzy 'refl' matches Reflow");
        assert!(p.rows.iter().all(|r| r.label.to_lowercase().contains('r')));
        // no match → empty
        let mut p2 = Palette { query: "zzzzzz".into(), ..Default::default() };
        rebuild_rows(&mut p2, &reg, &keymap);
        assert!(p2.rows.is_empty());
    }
}
