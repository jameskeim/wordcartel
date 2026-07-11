//! Linear undo/redo. Reimplemented from Helix/CodeMirror history patterns
//! (MPL/MIT) — pattern, not copied source (spec §9.6). v1 is linear (no branch).
use crate::buffer::TextBuffer;
use crate::change::ChangeSet;
use crate::selection::Selection;

/// Coalescing window, in milliseconds: consecutive `Type`-kind edits committed via
/// [`History::commit_coalescing`] merge into the same undo revision as long as each new edit
/// lands within this many milliseconds of the previous one. Edits farther apart, or of any
/// other [`EditKind`], start a fresh revision.
pub const COALESCE_MS: u64 = 500;
/// Undo-history memory budget: evict oldest revisions past this, always keeping ≥1 (M5).
pub const MAX_UNDO_BYTES: usize = 64 * 1024 * 1024;

/// A wall-clock time source for [`History::commit_coalescing`]'s coalescing-window check.
/// Abstracted behind a trait so tests can drive it with a fake, deterministic clock instead
/// of the real system time.
pub trait Clock {
    /// Returns the current time in milliseconds on whatever epoch the implementation
    /// chooses — only the *difference* between successive readings is meaningful to the
    /// caller, so the epoch itself never needs to be documented or stable.
    fn now_ms(&self) -> u64;
}

/// Classifies a committed edit for coalescing purposes. Only same-kind [`Type`](EditKind::Type)
/// edits inside the [`COALESCE_MS`] window may merge into one undo revision; every other kind
/// always starts a new revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditKind {
    /// A single incremental keystroke (typing a character). Eligible to coalesce with an
    /// immediately preceding `Type` edit within [`COALESCE_MS`].
    Type,
    /// Any non-typing edit — paste, programmatic change, formatting action, etc. Never
    /// coalesces with a neighbor, and its arrival ends any in-progress coalescing group.
    Other,
}

/// One atomic forward/backward change pair stored inside a [`Revision`]. Redo re-applies
/// `changes` to the buffer; undo applies `inverse` instead, restoring the prior text.
#[derive(Clone, Debug)]
pub struct Edit {
    /// The forward change — applied to the buffer when this edit is (re)done.
    pub changes: ChangeSet,
    /// The inverse of `changes` — applied to the buffer when this edit is undone.
    pub inverse: ChangeSet,
}

/// A proposed edit, plus an optional selection to leave behind, ready to hand to
/// [`History::commit`] or [`History::commit_coalescing`]. Built with [`Transaction::new`] and,
/// optionally, [`Transaction::with_selection`].
#[derive(Clone, Debug)]
pub struct Transaction {
    /// The change set to apply to the buffer.
    pub changes: ChangeSet,
    /// The selection to restore after `changes` is applied, if the caller supplied one via
    /// [`Transaction::with_selection`]. When `None`, the committing `History` method falls
    /// back to mapping the pre-edit selection through `changes`.
    pub selection: Option<Selection>,
}

impl Transaction {
    /// Creates a transaction from `changes` with no explicit post-edit selection — the
    /// committing `History` method will derive one by mapping the caller's pre-edit selection
    /// through `changes`. Use [`Transaction::with_selection`] to override that default.
    ///
    /// # Examples
    ///
    /// ```
    /// use wordcartel_core::change::ChangeSet;
    /// use wordcartel_core::history::Transaction;
    ///
    /// let changes = ChangeSet::insert(0, "hi", 0);
    /// let txn = Transaction::new(changes);
    /// assert!(txn.selection.is_none());
    /// ```
    pub fn new(changes: ChangeSet) -> Self {
        Transaction { changes, selection: None }
    }

    /// Sets the selection to restore once this transaction is applied, overriding the
    /// default mapped-selection behavior. Consumes and returns `self` for chaining onto
    /// [`Transaction::new`].
    pub fn with_selection(mut self, sel: Selection) -> Self {
        self.selection = Some(sel);
        self
    }
}

/// One undo/redo unit: one or more coalesced [`Edit`]s applied together as a group, plus the
/// selection state on either side of the group, the clock reading it was last extended at, and
/// the [`EditKind`] governing whether it may absorb further edits.
#[derive(Clone, Debug)]
pub struct Revision {
    /// The edits making up this revision, in application order. Undo replays their `inverse`s
    /// in reverse order; redo replays their `changes` in forward order.
    pub edits: Vec<Edit>,
    /// The selection immediately before this revision's first edit was applied — restored by
    /// [`History::undo`].
    pub before: Selection,
    /// The selection immediately after this revision's last edit was applied — restored by
    /// [`History::redo`].
    pub after: Selection,
    /// Clock reading (per [`Clock::now_ms`]) at which this revision was created or last
    /// extended by coalescing; compared against [`COALESCE_MS`] to decide whether a later
    /// `Type` edit may still merge into it.
    pub last_ms: u64,
    /// The kind of edit this revision holds — only a `Type` revision is eligible to absorb
    /// further coalesced edits.
    pub kind: EditKind,
}

/// Linear undo/redo stack for a single buffer (v1 has no branching history — see the module
/// docs). Revisions at indices `0..current` form the undo stack; any past `current` form the
/// redo tail. `bytes`/`last_evicted` track the M5 memory-budget accounting enforced by the
/// commit methods.
#[derive(Clone, Debug, Default)]
pub struct History {
    /// All retained revisions, oldest first. `revisions[..current]` is the undo stack;
    /// `revisions[current..]` is the redo tail.
    pub revisions: Vec<Revision>,
    /// Number of revisions currently applied — the boundary between the undo stack
    /// (`revisions[..current]`) and the redo tail (`revisions[current..]`).
    pub current: usize,       // number of revisions currently applied
    /// Running total of stored bytes across all retained revisions (M5 memory accounting),
    /// kept incrementally in sync by `commit`, `commit_coalescing`, and eviction.
    pub bytes: usize,         // running total of retained revisions' stored bytes (M5)
    /// Number of revisions evicted by the most recent [`History::commit`] or
    /// [`History::commit_coalescing`] call. Reset to `0` at the start of every commit, undo,
    /// and redo call, since only a commit can evict — a stale nonzero value never survives
    /// past the next call of any kind.
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

    /// Reverts the most recently applied revision, applying its edits' `inverse` changes to
    /// `buf` in reverse order, and returns the selection to restore (the revision's `before`
    /// selection). Returns `None`, leaving `buf` untouched, if there is nothing to undo (the
    /// undo stack is empty).
    ///
    /// # Examples
    ///
    /// ```
    /// use wordcartel_core::buffer::TextBuffer;
    /// use wordcartel_core::change::ChangeSet;
    /// use wordcartel_core::history::{History, Transaction};
    /// use wordcartel_core::selection::Selection;
    ///
    /// let mut buf = TextBuffer::from_str("");
    /// let mut hist = History::default();
    /// let sel = Selection::single(0);
    ///
    /// let changes = ChangeSet::insert(0, "hi", buf.len());
    /// hist.commit(Transaction::new(changes), &mut buf, sel);
    /// assert_eq!(buf.to_string(), "hi");
    ///
    /// let restored = hist.undo(&mut buf).unwrap();
    /// assert_eq!(buf.to_string(), "");
    /// assert_eq!(restored, Selection::single(0));
    /// assert!(hist.undo(&mut buf).is_none()); // nothing left to undo
    /// ```
    pub fn undo(&mut self, buf: &mut TextBuffer) -> Option<Selection> {
        self.last_evicted = 0; // undo commits nothing — keep the eviction transient honest
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

    /// Re-applies the next revision in the redo tail (the one just past `current`) to `buf`,
    /// via its edits' forward `changes` in application order, and returns the selection to
    /// restore (the revision's `after` selection). Returns `None`, leaving `buf` untouched, if
    /// there is nothing to redo (the redo tail is empty — either nothing was undone, or a new
    /// commit already cleared it).
    pub fn redo(&mut self, buf: &mut TextBuffer) -> Option<Selection> {
        self.last_evicted = 0; // redo commits nothing — keep the eviction transient honest
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

    /// Applies `txn` to `buf` like [`History::commit`], but — instead of always pushing a new
    /// revision — merges it into the top-of-stack revision when all of these hold: the redo
    /// tail is empty, the top revision's kind and this commit's `kind` are both
    /// [`EditKind::Type`], and `clock.now_ms()` is within [`COALESCE_MS`] of the top revision's
    /// `last_ms`. Otherwise it behaves exactly like `commit`, pushing a new revision of `kind`
    /// and clearing any redo tail. Either way, returns the new selection (`txn.selection` if
    /// set, else `before` mapped through `txn.changes`).
    ///
    /// # Examples
    ///
    /// ```
    /// use wordcartel_core::buffer::TextBuffer;
    /// use wordcartel_core::change::ChangeSet;
    /// use wordcartel_core::history::{Clock, EditKind, History, Transaction};
    /// use wordcartel_core::selection::Selection;
    ///
    /// struct FixedClock(u64);
    /// impl Clock for FixedClock {
    ///     fn now_ms(&self) -> u64 { self.0 }
    /// }
    ///
    /// let mut buf = TextBuffer::from_str("");
    /// let mut hist = History::default();
    /// let mut sel = Selection::single(0);
    ///
    /// // Two `Type` edits close together in time coalesce into one revision.
    /// let cs_a = ChangeSet::insert(0, "a", buf.len());
    /// sel = hist.commit_coalescing(Transaction::new(cs_a), &mut buf, sel, &FixedClock(0), EditKind::Type);
    /// let cs_b = ChangeSet::insert(1, "b", buf.len());
    /// hist.commit_coalescing(Transaction::new(cs_b), &mut buf, sel, &FixedClock(100), EditKind::Type);
    /// assert_eq!(buf.to_string(), "ab");
    /// assert_eq!(hist.revisions.len(), 1); // one undo step covers both keystrokes
    ///
    /// hist.undo(&mut buf);
    /// assert_eq!(buf.to_string(), "");
    /// ```
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
    fn undo_and_redo_reset_last_evicted() {
        use crate::buffer::TextBuffer;
        let mut hist = History::default();
        let mut buf = TextBuffer::from_str("");
        let mut sel = Selection::single(0);
        for _ in 0..3 {
            let at = buf.len();
            let cs = ChangeSet::from_ops(vec![Op::Retain(at), Op::Insert("zzz".into())], at);
            sel = hist.commit(Transaction::new(cs), &mut buf, sel.clone());
        }
        hist.evict_to(5);
        assert!(hist.last_evicted > 0, "precondition: eviction happened");
        hist.undo(&mut buf);
        assert_eq!(hist.last_evicted, 0, "undo commits nothing → resets last_evicted");
        hist.last_evicted = 2; // re-arm the transient by hand
        hist.redo(&mut buf);
        assert_eq!(hist.last_evicted, 0, "redo commits nothing → resets last_evicted");
        // Placement proof: a NO-OP undo (nothing to undo) still resets, because the
        // reset precedes the `current == 0` early-return guard.
        let mut h2 = History { last_evicted: 5, ..History::default() };
        h2.undo(&mut buf);
        assert_eq!(h2.last_evicted, 0, "no-op undo still consumes stale eviction state");
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
                prop_assert_eq!(redo_sel.unwrap(), sel_after_all);
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
            // Coalescing must actually FIRE: the whole within-window run of same-kind
            // edits collapses into ONE revision (one undo unit). Without this, "full
            // undo restores the original" would pass vacuously even if each edit landed
            // in its own revision — so assert the run is a single revision.
            prop_assert_eq!(hist.revisions.len(), 1);
            // full undo must restore the exact original text — no chars lost
            while hist.undo(&mut buf).is_some() {}
            prop_assert_eq!(buf.slice(0..buf.len()), original);
        }
    }
}
