//! Linear undo/redo. Reimplemented from Helix/CodeMirror history patterns
//! (MPL/MIT) — pattern, not copied source (spec §9.6). v1 is linear (no branch).
use crate::buffer::TextBuffer;
use crate::change::ChangeSet;
use crate::selection::Selection;

#[derive(Clone, Debug)]
pub struct Edit {
    pub changes: ChangeSet,
    pub inverse: ChangeSet,
}

#[derive(Clone, Debug)]
pub struct Transaction {
    pub changes: ChangeSet,
    pub selection: Option<Selection>,
}

impl Transaction {
    pub fn new(changes: ChangeSet) -> Self {
        Transaction { changes, selection: None }
    }
    pub fn with_selection(mut self, sel: Selection) -> Self {
        self.selection = Some(sel);
        self
    }
}

#[derive(Clone, Debug)]
pub struct Revision {
    pub edits: Vec<Edit>,
    pub before: Selection,
    pub after: Selection,
}

#[derive(Clone, Debug, Default)]
pub struct History {
    pub revisions: Vec<Revision>,
    pub current: usize, // number of revisions currently applied
}

impl History {
    /// Apply `txn` to `buf`, record it as a new revision, and return the new
    /// selection. Clears any redo tail.
    pub fn commit(&mut self, txn: Transaction, buf: &mut TextBuffer, before: Selection) -> Selection {
        let inverse = txn.changes.invert(buf);
        txn.changes.apply(buf);
        let after = txn
            .selection
            .clone()
            .unwrap_or_else(|| before.map(&txn.changes));
        // drop any redo tail
        self.revisions.truncate(self.current);
        self.revisions.push(Revision {
            edits: vec![Edit { changes: txn.changes, inverse }],
            before,
            after: after.clone(),
        });
        self.current += 1;
        after
    }

    pub fn undo(&mut self, buf: &mut TextBuffer) -> Option<Selection> {
        if self.current == 0 {
            return None;
        }
        self.current -= 1;
        let rev = &self.revisions[self.current];
        for edit in rev.edits.iter().rev() {
            edit.inverse.apply(buf);
        }
        Some(rev.before.clone())
    }

    pub fn redo(&mut self, buf: &mut TextBuffer) -> Option<Selection> {
        if self.current >= self.revisions.len() {
            return None;
        }
        let rev = &self.revisions[self.current];
        for edit in rev.edits.iter() {
            edit.changes.apply(buf);
        }
        self.current += 1;
        Some(rev.after.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::ChangeSet;
    use crate::selection::Selection;

    fn type_char(buf: &TextBuffer, at: usize, ch: &str) -> Transaction {
        let cs = ChangeSet::insert(at, ch, buf.len());
        Transaction::new(cs).with_selection(Selection::single(at + ch.len()))
    }

    #[test]
    fn undo_then_redo_round_trip() {
        let mut buf = TextBuffer::from_str("");
        let mut hist = History::default();
        let mut sel = Selection::single(0);

        sel = hist.commit(type_char(&buf, 0, "a"), &mut buf, sel.clone());
        // apply happens inside commit; re-fetch buffer state by applying in commit
        // (commit applies the txn to buf)
        assert_eq!(buf.to_string(), "a");
        sel = hist.commit(type_char(&buf, 1, "b"), &mut buf, sel.clone());
        assert_eq!(buf.to_string(), "ab");

        let s = hist.undo(&mut buf).unwrap();
        assert_eq!(buf.to_string(), "a");
        assert_eq!(s, Selection::single(1)); // before-selection of the 'b' revision

        let s2 = hist.redo(&mut buf).unwrap();
        assert_eq!(buf.to_string(), "ab");
        assert_eq!(s2, Selection::single(2));

        let _ = sel;
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut buf = TextBuffer::from_str("");
        let mut hist = History::default();
        let sel = Selection::single(0);
        let sel = hist.commit(type_char(&buf, 0, "a"), &mut buf, sel);
        hist.undo(&mut buf);
        assert_eq!(buf.to_string(), "");
        // a new edit after undo clears the redo stack
        hist.commit(type_char(&buf, 0, "z"), &mut buf, sel);
        assert_eq!(buf.to_string(), "z");
        assert!(hist.redo(&mut buf).is_none());
    }
}
