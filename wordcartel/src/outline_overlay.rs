//! Fuzzy heading picker overlay. XOR with other overlays and bound to a buffer id.

use ropey::Rope;
use wordcartel_core::block_tree::BlockTree;

#[derive(Debug, Clone)]
pub struct OutlineRow {
    pub byte: usize,
    pub indent: usize,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct OutlineOverlay {
    pub buffer_id: crate::editor::BufferId,
    pub query: String,
    pub cursor: usize,
    pub rows: Vec<OutlineRow>,
    pub selected: usize,
    all: Vec<OutlineRow>,
}

impl OutlineOverlay {
    pub fn open(buffer_id: crate::editor::BufferId, blocks: &BlockTree, rope: &Rope) -> OutlineOverlay {
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
            query: String::new(),
            cursor: 0,
            rows: all.clone(),
            selected: 0,
            all,
        }
    }

    pub fn set_query(&mut self, q: &str, _blocks: &BlockTree, _rope: &Rope) {
        self.query = q.to_string();
        self.cursor = self.query.len();
        self.rows = crate::palette::fuzzy_filter(&self.all, q, |r| &r.text);
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn overlay_lists_headings_indented_and_filters() {
        let doc = "# Top\n## Alpha\n## Beta\n### Beta1\n";
        let buf = wordcartel_core::buffer::TextBuffer::from_str(doc);
        let blocks = wordcartel_core::block_tree::full_parse_rope(&buf.snapshot());
        let mut ov = super::OutlineOverlay::open(crate::editor::BufferId(7), &blocks, &buf.snapshot());
        assert_eq!(ov.rows.len(), 4);
        assert_eq!(ov.rows[0].indent, 0);
        assert_eq!(ov.rows[3].indent, 2);
        ov.set_query("beta", &blocks, &buf.snapshot());
        assert!(ov.rows.iter().all(|r| r.text.to_lowercase().contains("beta")));
    }
}
