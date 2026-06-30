//! Linear undo/redo. Reimplemented from Helix/CodeMirror history patterns
//! (MPL/MIT) — pattern, not copied source (spec §9.6). v1 is linear (no branch).
use crate::buffer::TextBuffer;
use crate::change::ChangeSet;
use crate::selection::Selection;

pub const COALESCE_MS: u64 = 500;
/// Undo-history memory budget: evict oldest revisions past this, always keeping ≥1 (M5).
pub const MAX_UNDO_BYTES: usize = 64 * 1024 * 1024;

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
    pub current: usize,       // number of revisions currently applied
    pub bytes: usize,         // running total of retained revisions' stored bytes (M5)
    pub last_evicted: usize,  // revisions dropped on the most recent commit (M5)
}

fn revision_bytes(rev: &Revision) -> usize {
    rev.edits.iter().map(|e| e.changes.stored_bytes() + e.inverse.stored_bytes()).sum()
}

impl History {
    /// Evict oldest revisions while over `budget`, keeping ≥1. Sets `last_evicted`. MUST run only
    /// right after a commit (where `current == revisions.len()`), so decrementing `current` per
    /// front-eviction keeps undo/redo indices valid.
    fn evict_to(&mut self, budget: usize) {
        self.last_evicted = 0;
        while self.bytes > budget && self.revisions.len() > 1 {
            let rev = self.revisions.remove(0);
            self.bytes = self.bytes.saturating_sub(revision_bytes(&rev));
            self.current = self.current.saturating_sub(1);
            self.last_evicted += 1;
        }
    }

    /// Apply `txn` to `buf`, record it as a new revision, and return the new
    /// selection. Clears any redo tail.
    pub fn commit(&mut self, txn: Transaction, buf: &mut TextBuffer, before: Selection) -> Selection {
        let inverse = txn.changes.invert(buf);
        txn.changes.apply(buf);
        let after = txn.selection.clone().unwrap_or_else(|| before.map(&txn.changes));
        // Drop the redo tail — subtract its bytes FIRST so `bytes` stays accurate.
        let tail: usize = self.revisions[self.current..].iter().map(revision_bytes).sum();
        self.bytes = self.bytes.saturating_sub(tail);
        self.revisions.truncate(self.current);
        let rev = Revision {
            edits: vec![Edit { changes: txn.changes, inverse }],
            before, after: after.clone(), last_ms: 0, kind: EditKind::Other,
        };
        self.bytes += revision_bytes(&rev);
        self.revisions.push(rev);
        self.current += 1;
        self.evict_to(MAX_UNDO_BYTES);
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
            let (old, new);
            {
                let top = self.revisions.last_mut().unwrap();
                old = revision_bytes(top);
                top.edits.push(Edit { changes: txn.changes, inverse });
                top.after = after.clone();
                top.last_ms = now;
                new = revision_bytes(top);
            }
            self.bytes = self.bytes - old + new; // subtract-then-add avoids any underflow path
        } else {
            let tail: usize = self.revisions[self.current..].iter().map(revision_bytes).sum();
            self.bytes = self.bytes.saturating_sub(tail);
            self.revisions.truncate(self.current);
            let rev = Revision {
                edits: vec![Edit { changes: txn.changes, inverse }],
                before, after: after.clone(), last_ms: now, kind,
            };
            self.bytes += revision_bytes(&rev);
            self.revisions.push(rev);
            self.current += 1;
        }
        self.evict_to(MAX_UNDO_BYTES);
        after
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::change::{ChangeSet, Op};
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

    // ── Task 2 (M5): byte accounting + eviction ───────────────────────────────

    #[test]
    fn eviction_keeps_current_consistent_for_undo_redo() {
        use crate::buffer::TextBuffer;
        let mut hist = History::default();
        let mut buf = TextBuffer::from_str("");
        let mut sel = Selection::single(0);
        for _ in 0..3 {
            let at = buf.len();
            let cs = ChangeSet::from_ops(vec![Op::Retain(at), Op::Insert("zzz".into())], at);
            sel = hist.commit(Transaction::new(cs), &mut buf, sel.clone());
        }
        // 3 revisions, stored_bytes 3 each (the "zzz" insert) → bytes == 9. Force eviction to ≤5.
        hist.evict_to(5);
        assert!(hist.last_evicted > 0, "oldest revisions must have been evicted");
        assert!(!hist.revisions.is_empty(), "keep at least one");
        assert_eq!(hist.current, hist.revisions.len(), "current must equal len after evict");
        // The critical invariant: undo then redo round-trips without panicking / mis-indexing.
        let pre = buf.to_string();
        hist.undo(&mut buf);
        hist.redo(&mut buf);
        assert_eq!(buf.to_string(), pre, "undo+redo round-trips after eviction");
    }

    #[test]
    fn single_over_budget_revision_is_retained() {
        use crate::buffer::TextBuffer;
        let mut hist = History::default();
        let mut buf = TextBuffer::from_str("");
        let cs = ChangeSet::from_ops(vec![Op::Insert("hello".into())], 0);
        hist.commit(Transaction::new(cs), &mut buf, Selection::single(0));
        hist.evict_to(0); // budget 0, but keep-≥1 means the lone revision stays
        assert_eq!(hist.revisions.len(), 1);
        assert_eq!(hist.last_evicted, 0);
    }

    #[test]
    fn bytes_accounting_accurate_after_redo_tail_truncation() {
        use crate::buffer::TextBuffer;
        let mut hist = History::default();
        let mut buf = TextBuffer::from_str("");
        let sel = Selection::single(0);
        // commit "abc"
        let cs1 = ChangeSet::from_ops(vec![Op::Insert("abc".into())], 0);
        let sel = hist.commit(Transaction::new(cs1), &mut buf, sel);
        // undo to create a redo tail
        hist.undo(&mut buf);
        // commit new edit, truncating the redo tail
        let cs2 = ChangeSet::from_ops(vec![Op::Insert("xy".into())], 0);
        hist.commit(Transaction::new(cs2), &mut buf, sel);
        // bytes must equal fresh recompute
        let expected: usize = hist.revisions.iter().map(revision_bytes).sum();
        assert_eq!(hist.bytes, expected, "bytes must match fresh recompute after redo-tail truncation");
    }

    #[test]
    fn normal_session_never_evicts() {
        use crate::buffer::TextBuffer;
        let mut hist = History::default();
        let mut buf = TextBuffer::from_str("");
        let mut sel = Selection::single(0);
        // a handful of small edits — well within 64 MiB
        for i in 0..5usize {
            let at = buf.len();
            let text = format!("word{}", i);
            let cs = ChangeSet::from_ops(vec![Op::Retain(at), Op::Insert(text.as_str().into())], at);
            sel = hist.commit(Transaction::new(cs), &mut buf, sel.clone());
        }
        assert_eq!(hist.last_evicted, 0, "small edits must not trigger eviction");
        assert!(hist.bytes < 1024, "accumulated bytes for tiny edits must be negligible");
    }

    // ── Task 3 (M7): T4 History undo/redo proptests ───────────────────────────
    use proptest::prelude::*;
    use crate::proptest_strategies::prop_unicode_string;
    use crate::test_support::snap;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        #[test]
        fn t4_undo_redo_round_trips_exact(
            text in prop_unicode_string(),
            edits in proptest::collection::vec((0usize..60, prop_unicode_string()), 1..8),
        ) {
            use crate::buffer::TextBuffer;
            let mut buf = TextBuffer::from_str(&text);
            let mut hist = History::default();
            let mut sel = Selection::single(0);
            for (p, ins) in &edits {
                let at = snap(&buf.slice(0..buf.len()), (*p).min(buf.len()));
                let cs = ChangeSet::insert(at, ins, buf.len());
                sel = hist.commit(Transaction::new(cs), &mut buf, sel.clone());
                // selection valid: in-bounds + on char boundary
                prop_assert!(sel.primary().head <= buf.len());
                prop_assert!(buf.slice(0..buf.len()).is_char_boundary(sel.primary().head));
            }
            let after_all = buf.slice(0..buf.len());
            let sel_after_all = sel.clone();
            // undo then redo returns the exact BUFFER and the exact SELECTION (redo returns the
            // post-edit `after` selection — assert it, don't drop it — the spec's "undo->redo exact").
            if hist.undo(&mut buf).is_some() {
                let redo_sel = hist.redo(&mut buf);
                prop_assert_eq!(buf.slice(0..buf.len()), after_all);
                prop_assert_eq!(redo_sel.unwrap().primary().head, sel_after_all.primary().head);
            }
            // full undo yields the original
            while hist.undo(&mut buf).is_some() {}
            prop_assert_eq!(buf.slice(0..buf.len()), text);
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        #[test]
        fn t4_coalescing_loses_nothing(
            text in ".{0,20}",
            chars in proptest::collection::vec(
                proptest::sample::select(vec!["a", "b", "é", "中", "🙂"]),
                1..8usize,
            ),
        ) {
            use crate::buffer::TextBuffer;
            let original = text.clone();
            let mut buf = TextBuffer::from_str(&text);
            let mut hist = History::default();
            let clock = FakeClock::new();
            let mut sel = Selection::single(0);
            // all edits at 100ms intervals — always within the 500ms coalesce window
            for (i, ch) in chars.iter().enumerate() {
                clock.set(i as u64 * 100);
                let at = sel.primary().head;
                let cs = ChangeSet::insert(at, ch, buf.len());
                let txn = Transaction::new(cs)
                    .with_selection(Selection::single(at + ch.len()));
                sel = hist.commit_coalescing(txn, &mut buf, sel.clone(), &clock, EditKind::Type);
            }
            // full undo must restore the exact original text — no chars lost
            while hist.undo(&mut buf).is_some() {}
            prop_assert_eq!(buf.slice(0..buf.len()), original);
        }
    }
}
