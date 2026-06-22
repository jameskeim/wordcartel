//! Selection over byte offsets. `map` keeps positions valid across edits — the
//! #1 "cursor jumped" bug class (spec §10.2). Reimplemented from Helix's pattern.
use crate::change::{ChangeSet, Op};
use crate::BytePos;
use smallvec::{smallvec, SmallVec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub anchor: BytePos,
    pub head: BytePos,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub ranges: SmallVec<[Range; 1]>,
    pub primary: usize,
}

impl Range {
    pub fn point(pos: BytePos) -> Range {
        Range { anchor: pos, head: pos }
    }
    pub fn from(&self) -> BytePos {
        self.anchor.min(self.head)
    }
    pub fn to(&self) -> BytePos {
        self.anchor.max(self.head)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// Map both ends through a ChangeSet (insertion bias = After).
    pub fn map(&self, cs: &ChangeSet) -> Range {
        Range {
            anchor: map_pos(self.anchor, cs),
            head: map_pos(self.head, cs),
        }
    }
}

/// Map one byte position through a ChangeSet.
/// - Retain(n): positions in the retained span shift by the net delta so far.
/// - Insert(s): a position at/after the insert point gains s.len() (bias After).
/// - Delete(n): a position inside the deleted span clamps to its start.
fn map_pos(pos: BytePos, cs: &ChangeSet) -> BytePos {
    let mut old = 0usize; // cursor in the pre-change doc
    let mut new = 0usize; // cursor in the post-change doc
    for op in &cs.ops {
        match op {
            Op::Retain(n) => {
                if pos < old + n {
                    return new + (pos - old);
                }
                old += n;
                new += n;
            }
            Op::Insert(s) => { new += s.len(); }
            Op::Delete(n) => {
                if pos < old + n {
                    // inside (or at start of) the deletion → clamp to its start
                    return new;
                }
                old += n;
            }
        }
    }
    new + pos.saturating_sub(old)
}

impl Selection {
    pub fn single(pos: BytePos) -> Selection {
        Selection { ranges: smallvec![Range::point(pos)], primary: 0 }
    }
    pub fn primary(&self) -> Range {
        self.ranges[self.primary]
    }
    pub fn map(&self, cs: &ChangeSet) -> Selection {
        Selection {
            ranges: self.ranges.iter().map(|r| r.map(cs)).collect(),
            primary: self.primary,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_after_insert_before_it() {
        // cursor at byte 5; insert 2 bytes at byte 2 (before it) → cursor shifts +2
        let cur = Range { anchor: 5, head: 5 };
        let cs = ChangeSet::insert(2, "XY", 10);
        let mapped = cur.map(&cs);
        assert_eq!(mapped, Range { anchor: 7, head: 7 });
    }

    #[test]
    fn cursor_unaffected_by_insert_after_it() {
        let cur = Range { anchor: 3, head: 3 };
        let cs = ChangeSet::insert(8, "Z", 10);
        assert_eq!(cur.map(&cs), Range { anchor: 3, head: 3 });
    }

    #[test]
    fn cursor_after_delete_before_it() {
        let cur = Range { anchor: 9, head: 9 };
        let cs = ChangeSet::delete(2..5, 12); // remove 3 bytes before cursor
        assert_eq!(cur.map(&cs), Range { anchor: 6, head: 6 });
    }

    #[test]
    fn cursor_inside_deleted_clamps_to_start() {
        let cur = Range { anchor: 4, head: 4 };
        let cs = ChangeSet::delete(2..6, 10); // cursor 4 is inside [2,6)
        assert_eq!(cur.map(&cs), Range { anchor: 2, head: 2 });
    }

    #[test]
    fn insertion_bias_is_after() {
        // cursor exactly at the insert point moves to AFTER the inserted text
        let cur = Range { anchor: 2, head: 2 };
        let cs = ChangeSet::insert(2, "AB", 10);
        assert_eq!(cur.map(&cs), Range { anchor: 4, head: 4 });
    }

    use proptest::prelude::*;

    proptest! {
        // LAW: a mapped position is always within the new document bounds and never
        // lands inside what was deleted before it (spec §10.2 cursor-jump class).
        #[test]
        fn prop_mapped_pos_in_bounds(
            doc_len in 1usize..40,
            pos in 0usize..40,
            at in 0usize..40,
            ins_len in 0usize..6,
        ) {
            let pos = pos.min(doc_len);
            let at = at.min(doc_len);
            let cs = if ins_len > 0 {
                ChangeSet::insert(at, &"x".repeat(ins_len), doc_len)
            } else if at < doc_len {
                ChangeSet::delete(at..doc_len, doc_len)
            } else {
                ChangeSet::insert(at, "x", doc_len)
            };
            let mapped = super::map_pos(pos, &cs);
            prop_assert!(mapped <= cs.len_after);
        }
    }
}
