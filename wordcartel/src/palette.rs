use crate::registry::{Registry, CommandId};
use crate::keymap::KeyTrie;
use crate::editor::BufferId;
use crate::app::Msg;
use crossterm::event::Event;
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

/// Palette overlay intercepts KEY INPUT and PASTE. Non-key, non-paste messages
/// (FilterDone, JobDone, Tick) fall through to normal handling while the
/// palette stays open.
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ctx: &crate::overlays::DispatchCtx) -> crate::app::Handled {
    if editor.palette.is_none() { return crate::app::Handled::Pass(msg); }
    if matches!(&msg, Msg::ClipboardPaste { .. }) {
        // Drop an async clipboard-paste result that arrives while the palette is
        // open — it must not land in the document behind the overlay.
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
    }
    if let Msg::Input(Event::Paste(text)) = msg {
        let ah = editor.active().view.area.1;
        if let Some(p) = editor.palette.as_mut() {
            p.query.insert_str(p.cursor, &text);
            p.cursor += text.len();
            crate::palette::rebuild_rows(p, ctx.reg, ctx.keymap);
            crate::app::keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
    }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            match k.code {
                crossterm::event::KeyCode::Esc => {
                    editor.palette = None;
                }
                crossterm::event::KeyCode::Enter => {
                    let row = editor.palette.as_ref()
                        .and_then(|p| p.rows.get(p.selected).cloned());
                    if let Some(row) = row {
                        if let Some(bid) = row.buffer {
                            // Buffer-switcher row: dismiss palette, jump to buffer.
                            editor.palette = None;
                            if let Some(idx) = editor.buffers.iter().position(|b| b.id == bid) {
                                crate::workspace::switch_to(editor, idx);
                            }
                        } else {
                            // Command-palette row: dispatch through registry.
                            crate::app::dispatch_overlay_command(editor, ctx.reg, ctx.keymap, ctx.ex, ctx.clock, ctx.msg_tx, row.id, ctx.fs);
                        }
                    }
                }
                c if crate::list_window::list_nav_key(c).is_some() => {
                    let ah = editor.active().view.area.1;
                    if let Some(p) = editor.palette.as_mut() {
                        crate::list_window::apply_list_nav(crate::list_window::list_nav_key(c).unwrap(),
                            ah, p.rows.len(), &mut p.selected, &mut p.scroll_top);
                    }
                }
                crossterm::event::KeyCode::Backspace => {
                    let ah = editor.active().view.area.1;
                    if let Some(p) = editor.palette.as_mut() {
                        if p.cursor > 0 {
                            // remove the char before cursor (byte-safe for ASCII labels)
                            let byte_pos = p.query[..p.cursor].char_indices().next_back().map(|(i, _)| i).unwrap_or(0);
                            p.query.remove(byte_pos);
                            p.cursor = byte_pos;
                        }
                        crate::palette::rebuild_rows(p, ctx.reg, ctx.keymap);
                        crate::app::keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                    }
                }
                crossterm::event::KeyCode::Left => {
                    if let Some(p) = editor.palette.as_mut() {
                        if p.cursor > 0 {
                            p.cursor -= p.query[..p.cursor].char_indices().next_back().map(|(_, c)| c.len_utf8()).unwrap_or(0);
                        }
                    }
                }
                crossterm::event::KeyCode::Right => {
                    if let Some(p) = editor.palette.as_mut() {
                        if p.cursor < p.query.len() {
                            let c = p.query[p.cursor..].chars().next().unwrap();
                            p.cursor += c.len_utf8();
                        }
                    }
                }
                crossterm::event::KeyCode::Char(c)
                    if !k.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        && !k.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
                {
                    let ah = editor.active().view.area.1;
                    if let Some(p) = editor.palette.as_mut() {
                        p.query.insert(p.cursor, c);
                        p.cursor += c.len_utf8();
                        crate::palette::rebuild_rows(p, ctx.reg, ctx.keymap);
                        crate::app::keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                    }
                }
                _ => {}
            }
        }
        return crate::app::Handled::Done(crate::app::fold_and_continue(editor, ctx.ex, ctx.clock, ctx.msg_tx, ctx.fs));
    }
    // Non-key msg falls through to normal handling while palette stays open.
    crate::app::Handled::Pass(msg)
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

    // -----------------------------------------------------------------------
    // LAW 3 (command-surface contract): palette is exhaustive over the registry
    // -----------------------------------------------------------------------

    #[test]
    fn palette_is_exhaustive_over_the_registry() {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = Palette::default();
        rebuild_rows(&mut p, &reg, &km);
        let ids: std::collections::HashSet<_> = p.rows.iter().map(|r| r.id).collect();
        for (id, _) in reg.commands() {
            assert!(ids.contains(&id), "palette missing registered command {}", id.0);
        }
        assert_eq!(p.rows.len(), reg.commands().count(), "row count == registry command count");
    }

    /// Effort P1 (spec §9): LAW 3 holds even with plugin entries present — plugin commands
    /// register into the SAME `Registry` the palette iterates (no parallel command store), so
    /// palette-completeness is structural, not vigilant.
    #[test]
    fn palette_is_exhaustive_over_a_plugin_loaded_registry() {
        let mut reg = crate::registry::Registry::builtins();
        let mut host = crate::plugin::host::PluginHost::new().expect("VM construction");
        let src = "wc.register_command{ name='insert', label='Plugin Insert', fn=function() end }";
        let reports = crate::plugin::load::load_sources(
            &mut reg, &mut host, &[("p1demo".to_string(), src.to_string())],
            &std::collections::BTreeMap::new(), &mut Vec::new(),
        );
        assert_eq!(reports[0].result, Ok(1), "test plugin must load cleanly: {:?}", reports[0].result);
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut p = Palette::default();
        rebuild_rows(&mut p, &reg, &km);
        let ids: std::collections::HashSet<_> = p.rows.iter().map(|r| r.id).collect();
        for (id, _) in reg.commands() {
            assert!(ids.contains(&id), "palette missing registered command {}", id.0);
        }
        assert_eq!(p.rows.len(), reg.commands().count(), "row count == registry command count");
        assert!(p.rows.iter().any(|r| r.label == "Plugin Insert"),
            "the plugin command must appear in the palette like any builtin");
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
