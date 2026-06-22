//! Linear undo/redo. Reimplemented from Helix/CodeMirror history patterns
//! (MPL/MIT) — pattern, not copied source (spec §9.6). v1 is linear (no branch).
use crate::buffer::TextBuffer;
use crate::change::ChangeSet;
use crate::selection::Selection;

pub const COALESCE_MS: u64 = 500;

pub trait Clock {
    fn now_ms(&self) -> u64;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    Type,
    Other,
}

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
    pub last_ms: u64,
    pub kind: EditKind,
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
            last_ms: 0,
            kind: EditKind::Other,
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

    pub fn commit_coalescing(
        &mut self,
        txn: Transaction,
        buf: &mut TextBuffer,
        before: Selection,
        clock: &dyn Clock,
        kind: EditKind,
    ) -> Selection {
        let now = clock.now_ms();
        let can_merge = self.current > 0
            && self.current == self.revisions.len() // nothing in redo tail
            && kind == EditKind::Type
            && {
                let top = &self.revisions[self.current - 1];
                top.kind == EditKind::Type && now.saturating_sub(top.last_ms) <= COALESCE_MS
            };

        let inverse = txn.changes.invert(buf);
        txn.changes.apply(buf);
        let after = txn
            .selection
            .clone()
            .unwrap_or_else(|| before.map(&txn.changes));

        if can_merge {
            let top = self.revisions.last_mut().unwrap();
            top.edits.push(Edit { changes: txn.changes, inverse });
            top.after = after.clone();
            top.last_ms = now;
        } else {
            self.revisions.truncate(self.current);
            self.revisions.push(Revision {
                edits: vec![Edit { changes: txn.changes, inverse }],
                before,
                after: after.clone(),
                last_ms: now,
                kind,
            });
            self.current += 1;
        }
        after
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

    struct FakeClock {
        t: std::cell::Cell<u64>,
    }
    impl FakeClock {
        fn new() -> Self { FakeClock { t: std::cell::Cell::new(0) } }
        fn set(&self, ms: u64) { self.t.set(ms); }
    }
    impl Clock for FakeClock {
        fn now_ms(&self) -> u64 { self.t.get() }
    }

    #[test]
    fn rapid_typing_coalesces_into_one_undo() {
        let mut buf = TextBuffer::from_str("");
        let mut hist = History::default();
        let clock = FakeClock::new();
        let mut sel = Selection::single(0);

        clock.set(0);
        sel = hist.commit_coalescing(type_char(&buf, 0, "a"), &mut buf, sel, &clock, EditKind::Type);
        clock.set(100); // within 500ms window
        sel = hist.commit_coalescing(type_char(&buf, 1, "b"), &mut buf, sel, &clock, EditKind::Type);
        clock.set(200);
        let _ = hist.commit_coalescing(type_char(&buf, 2, "c"), &mut buf, sel, &clock, EditKind::Type);
        assert_eq!(buf.to_string(), "abc");

        // one undo removes the whole "abc" burst
        let s = hist.undo(&mut buf).unwrap();
        assert_eq!(buf.to_string(), "");
        assert_eq!(s, Selection::single(0));
    }

    #[test]
    fn pause_breaks_the_group() {
        let mut buf = TextBuffer::from_str("");
        let mut hist = History::default();
        let clock = FakeClock::new();
        let mut sel = Selection::single(0);

        clock.set(0);
        sel = hist.commit_coalescing(type_char(&buf, 0, "a"), &mut buf, sel, &clock, EditKind::Type);
        clock.set(1000); // > 500ms later → new group
        let _ = hist.commit_coalescing(type_char(&buf, 1, "b"), &mut buf, sel, &clock, EditKind::Type);
        assert_eq!(buf.to_string(), "ab");

        // undo removes only "b"
        hist.undo(&mut buf);
        assert_eq!(buf.to_string(), "a");
    }

    #[test]
    fn non_type_edit_never_coalesces() {
        let mut buf = TextBuffer::from_str("");
        let mut hist = History::default();
        let clock = FakeClock::new();
        let mut sel = Selection::single(0);
        clock.set(0);
        sel = hist.commit_coalescing(type_char(&buf, 0, "a"), &mut buf, sel, &clock, EditKind::Type);
        clock.set(50);
        // a paste/programmatic edit (EditKind::Other) starts its own group even within the window
        let _ = hist.commit_coalescing(type_char(&buf, 1, "X"), &mut buf, sel, &clock, EditKind::Other);
        hist.undo(&mut buf);
        assert_eq!(buf.to_string(), "a"); // only the Other edit undone
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

    /// A coalesced burst of 3 Type chars forms one revision. After undo, a single
    /// redo must return the burst's *after* selection exactly (the position left by
    /// the last coalesced keystroke).
    #[test]
    fn redo_after_coalesced_burst_returns_after_selection() {
        let mut buf = TextBuffer::from_str("");
        let mut hist = History::default();
        let clock = FakeClock::new();
        let mut sel = Selection::single(0);

        clock.set(0);
        sel = hist.commit_coalescing(type_char(&buf, 0, "a"), &mut buf, sel, &clock, EditKind::Type);
        clock.set(100);
        sel = hist.commit_coalescing(type_char(&buf, 1, "b"), &mut buf, sel, &clock, EditKind::Type);
        clock.set(200);
        let after_burst = hist.commit_coalescing(type_char(&buf, 2, "c"), &mut buf, sel, &clock, EditKind::Type);
        assert_eq!(buf.to_string(), "abc");
        // after_burst is the selection after the last 'c' was typed (head = 3)
        assert_eq!(after_burst, Selection::single(3));

        // undo the whole burst in one step
        hist.undo(&mut buf).unwrap();
        assert_eq!(buf.to_string(), "");

        // redo must return exactly the after-selection of the burst
        let redone_sel = hist.redo(&mut buf).unwrap();
        assert_eq!(buf.to_string(), "abc");
        assert_eq!(redone_sel, after_burst, "redo should return the burst's after selection");
    }
}
