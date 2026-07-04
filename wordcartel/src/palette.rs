use crate::registry::{Registry, CommandId};
use crate::keymap::KeyTrie;
use crate::editor::BufferId;
use nucleo_matcher::{Matcher, Config};
use nucleo_matcher::pattern::{Pattern, CaseMatching, Normalization};

/// Discriminates which data source `rebuild_rows` consults.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PaletteKind {
    /// Normal command palette: rows come from the command registry.
    #[default]
    Commands,
    /// Buffer-switcher palette: rows come from `source_rows` (MRU list),
    /// and the registry is never consulted.
    Buffers,
}

#[derive(Default, Debug, Clone)]
pub struct Palette {
    pub query: String,
    pub cursor: usize,
    pub rows: Vec<PaletteRow>,
    pub selected: usize,
    /// Whether this palette shows registry commands or buffer entries.
    pub kind: PaletteKind,
    /// Unfiltered rows for the `Buffers` kind. Empty for `Commands`.
    pub source_rows: Vec<PaletteRow>,
    /// First visible row of the windowed list (A6). Maintained by keep_visible.
    pub scroll_top: usize,
}

#[derive(Debug, Clone)]
pub struct PaletteRow {
    pub id: CommandId,
    pub label: String,
    pub chord: String,
    /// For buffer-switcher rows: the buffer this row represents.
    /// `None` for normal command rows.
    pub buffer: Option<BufferId>,
}

/// Fuzzy-rank `items` against `query` by each item's key string, best-first.
/// Returns the matching items (cloned). Shared by the palette and the outline overlay.
pub fn fuzzy_filter<T: Clone>(items: &[T], query: &str, key: impl Fn(&T) -> &str) -> Vec<T> {
    if query.is_empty() {
        return items.to_vec();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pat = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut scored: Vec<(usize, u32, T)> = items.iter().enumerate()
        .filter_map(|(i, item)| {
            let mut buf = Vec::new();
            let hay = nucleo_matcher::Utf32Str::new(key(item), &mut buf);
            pat.score(hay, &mut matcher).map(|s| (i, s, item.clone()))
        }).collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

/// Rebuild the displayed rows from either the command registry (`Commands` kind)
/// or the pre-built `source_rows` list (`Buffers` kind), ranked by `query`.
///
/// * `Commands` — empty query → all registry commands in registration order;
///   non-empty query → fuzzy matches, score-desc, ties by registration order.
/// * `Buffers` — filters `source_rows` by label; registry is NOT consulted.
///
/// `selected` is clamped to the new row count in both cases.
pub fn rebuild_rows(p: &mut Palette, reg: &Registry, keymap: &KeyTrie) {
    match p.kind {
        PaletteKind::Commands => {
            let all: Vec<(CommandId, &str)> = reg.commands().map(|(id, m)| (id, m.label)).collect();
            let ranked: Vec<CommandId> = fuzzy_filter(&all, &p.query, |(_, label)| label)
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            p.rows = ranked.into_iter().map(|id| PaletteRow {
                id,
                label: reg.meta(id).map(|m| m.label.to_string()).unwrap_or_default(),
                chord: keymap.chord_for(id).unwrap_or_default(),
                buffer: None,
            }).collect();
        }
        PaletteKind::Buffers => {
            // Source rows already carry their BufferId; just fuzzy-filter by label.
            p.rows = fuzzy_filter(&p.source_rows, &p.query, |r| &r.label);
        }
    }
    if p.selected >= p.rows.len() { p.selected = p.rows.len().saturating_sub(1); }
    p.scroll_top = p.scroll_top.min(p.rows.len().saturating_sub(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_switcher_palette_filters_source_rows_not_registry() {
        let reg = crate::registry::Registry::builtins();
        let (keymap, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = Palette {
            kind: PaletteKind::Buffers,
            source_rows: vec![
                PaletteRow {
                    id: crate::registry::CommandId("palette"),
                    label: "notes.md".into(),
                    chord: "".into(),
                    buffer: Some(crate::editor::BufferId(1)),
                },
                PaletteRow {
                    id: crate::registry::CommandId("palette"),
                    label: "*scratch*".into(),
                    chord: "".into(),
                    buffer: Some(crate::editor::BufferId(2)),
                },
            ],
            ..Default::default()
        };
        rebuild_rows(&mut p, &reg, &keymap);
        // Empty query → all source rows, NOT registry commands
        assert_eq!(p.rows.len(), 2, "buffers palette shows source rows, not registry commands");
        assert_eq!(p.rows[0].buffer, Some(crate::editor::BufferId(1)));
        assert_eq!(p.rows[1].buffer, Some(crate::editor::BufferId(2)));
        // Fuzzy filter: "scratch" should match "*scratch*" and not "notes.md"
        p.query = "scratch".into();
        p.cursor = "scratch".len();
        rebuild_rows(&mut p, &reg, &keymap);
        assert_eq!(p.rows.len(), 1, "filter narrows to matching source rows only");
        assert_eq!(p.rows[0].buffer, Some(crate::editor::BufferId(2)));
        // No match → empty
        p.query = "zzznomatch".into();
        rebuild_rows(&mut p, &reg, &keymap);
        assert_eq!(p.rows.len(), 0);
    }

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
