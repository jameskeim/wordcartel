//! Fuzzy heading picker overlay. XOR with other overlays and bound to a buffer id.

use ropey::Rope;
use wordcartel_core::block_tree::BlockTree;
use crate::app::Msg;
use crossterm::event::Event;

#[derive(Debug, Clone)]
pub struct OutlineRow {
    pub byte: usize,
    pub indent: usize,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct OutlineOverlay {
    pub buffer_id: crate::editor::BufferId,
    pub opened_version: u64,
    pub query: String,
    pub cursor: usize,
    pub rows: Vec<OutlineRow>,
    pub selected: usize,
    /// First visible row index — drives the windowed painter (A6).
    pub scroll_top: usize,
    all: Vec<OutlineRow>,
}

impl OutlineOverlay {
    pub fn open(buffer_id: crate::editor::BufferId, opened_version: u64, blocks: &BlockTree, rope: &Rope) -> OutlineOverlay {
        let all: Vec<OutlineRow> = wordcartel_core::outline::headings(blocks, rope)
            .into_iter()
            .map(|h| OutlineRow {
                byte: h.byte,
                indent: (h.level as usize).saturating_sub(1),
                text: h.text,
            })
            .collect();
        OutlineOverlay {
            buffer_id,
            opened_version,
            query: String::new(),
            cursor: 0,
            rows: all.clone(),
            selected: 0,
            scroll_top: 0,
            all,
        }
    }

    pub fn set_query(&mut self, q: &str, _blocks: &BlockTree, _rope: &Rope) {
        self.query = q.to_string();
        self.cursor = self.query.len();
        self.rows = crate::palette::fuzzy_filter(&self.all, q, |r| &r.text);
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.scroll_top = self.scroll_top.min(self.rows.len().saturating_sub(1));
    }
}

pub fn outline_jump_to(editor: &mut crate::editor::Editor, byte: usize) {
    let origin = editor.active().document.selection.primary().head;
    crate::marks::record_jump(editor.active_mut(), origin);
    crate::registry::unfold_ancestors_of(editor, byte);
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(byte);
    editor.outline = None;
    crate::derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}

/// Outline overlay: XOR-closes on buffer switch (pre-close, BEFORE the `is_none()`
/// guard — §8.1-I), then intercepts KEY INPUT only; non-key messages fall through to
/// normal handling so background work is never starved while the overlay is open
/// (mirror of minibuffer/search/diag blocks above — 5e starvation lesson).
pub(crate) fn intercept(msg: crate::app::Msg, editor: &mut crate::editor::Editor,
    ex: &dyn crate::jobs::Executor, clock: &dyn wordcartel_core::history::Clock,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>) -> crate::app::Handled {
    if editor.outline.is_some()
        && editor.outline.as_ref().map(|o| o.buffer_id) != Some(editor.active().id) {
        editor.outline = None;
    }
    if editor.outline.is_none() { return crate::app::Handled::Pass(msg); }
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            use crossterm::event::{KeyCode, KeyModifiers};
            match k.code {
                KeyCode::Esc => { editor.outline = None; }
                KeyCode::Up => {
                    let ah = editor.active().view.area.1;
                    if let Some(o) = editor.outline.as_mut() {
                        o.selected = o.selected.saturating_sub(1);
                        crate::app::keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                    }
                }
                KeyCode::Down => {
                    let ah = editor.active().view.area.1;
                    if let Some(o) = editor.outline.as_mut() {
                        let max = o.rows.len().saturating_sub(1);
                        o.selected = (o.selected + 1).min(max);
                        crate::app::keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                    }
                }
                KeyCode::PageDown => {
                    let ah = editor.active().view.area.1;
                    if let Some(o) = editor.outline.as_mut() {
                        let lh = crate::list_window::list_h_for(o.rows.len(), ah);
                        o.selected = (o.selected + lh.max(1)).min(o.rows.len().saturating_sub(1));
                        crate::app::keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                    }
                }
                KeyCode::PageUp => {
                    let ah = editor.active().view.area.1;
                    if let Some(o) = editor.outline.as_mut() {
                        let lh = crate::list_window::list_h_for(o.rows.len(), ah);
                        o.selected = o.selected.saturating_sub(lh.max(1));
                        crate::app::keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                    }
                }
                KeyCode::Home => {
                    let ah = editor.active().view.area.1;
                    if let Some(o) = editor.outline.as_mut() {
                        o.selected = 0;
                        crate::app::keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                    }
                }
                KeyCode::End => {
                    let ah = editor.active().view.area.1;
                    if let Some(o) = editor.outline.as_mut() {
                        o.selected = o.rows.len().saturating_sub(1);
                        crate::app::keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                    }
                }
                KeyCode::Enter => {
                    if editor.outline.as_ref().map(|o| o.opened_version) != Some(editor.active().document.version) {
                        editor.status = "document changed; outline closed".into();
                        editor.outline = None;
                        return crate::app::Handled::Done({ for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); } !editor.quit });
                    }
                    let target = editor.outline.as_ref()
                        .and_then(|o| o.rows.get(o.selected))
                        .map(|r| r.byte);
                    if let Some(target) = target {
                        outline_jump_to(editor, target);
                    }
                }
                KeyCode::Backspace => {
                    let ah = editor.active().view.area.1;
                    if let Some(o) = editor.outline.as_mut() {
                        o.query.pop();
                    }
                    let q = editor.outline.as_ref().map(|o| o.query.clone()).unwrap_or_default();
                    let (blocks, rope) = { let b = editor.active(); (b.document.blocks().clone(), b.document.buffer.snapshot()) };
                    if let Some(o) = editor.outline.as_mut() {
                        o.set_query(&q, &blocks, &rope);
                        crate::app::keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                    }
                }
                KeyCode::Char(c)
                    if !k.modifiers.contains(KeyModifiers::CONTROL)
                        && !k.modifiers.contains(KeyModifiers::ALT) =>
                {
                    let ah = editor.active().view.area.1;
                    if let Some(o) = editor.outline.as_mut() {
                        o.query.push(c);
                    }
                    let q = editor.outline.as_ref().map(|o| o.query.clone()).unwrap_or_default();
                    let (blocks, rope) = { let b = editor.active(); (b.document.blocks().clone(), b.document.buffer.snapshot()) };
                    if let Some(o) = editor.outline.as_mut() {
                        o.set_query(&q, &blocks, &rope);
                        crate::app::keep_overlay_visible(ah, o.selected, o.rows.len(), &mut o.scroll_top);
                    }
                }
                _ => {}
            }
        }
        for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
        return crate::app::Handled::Done(!editor.quit);
    }
    // Non-key messages fall through to normal handlers below.
    crate::app::Handled::Pass(msg)
}

#[cfg(test)]
mod tests {
    #[test]
    fn overlay_lists_headings_indented_and_filters() {
        let doc = "# Top\n## Alpha\n## Beta\n### Beta1\n";
        let buf = wordcartel_core::buffer::TextBuffer::from_str(doc);
        let blocks = wordcartel_core::block_tree::full_parse_rope(&buf.snapshot());
        let mut ov = super::OutlineOverlay::open(crate::editor::BufferId(7), 0, &blocks, &buf.snapshot());
        assert_eq!(ov.rows.len(), 4);
        assert_eq!(ov.rows[0].indent, 0);
        assert_eq!(ov.rows[3].indent, 2);
        ov.set_query("beta", &blocks, &buf.snapshot());
        assert!(ov.rows.iter().all(|r| r.text.to_lowercase().contains("beta")));
    }
}
