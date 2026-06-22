//! ChangeSet: reversible byte-diff. Reimplemented from the Helix/CodeMirror
//! transaction pattern (MPL/MIT) — pattern, not copied source (spec §9.6).
use crate::buffer::TextBuffer;
use crate::BytePos;
use std::ops::Range;

pub type Tendril = smartstring::alias::String;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Retain(usize), // bytes
    Delete(usize), // bytes
    Insert(Tendril),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSet {
    pub ops: Vec<Op>,
    pub len_before: usize,
    pub len_after: usize,
}

impl ChangeSet {
    /// Insert `text` at byte offset `at` in a document of length `doc_len`.
    pub fn insert(at: BytePos, text: &str, doc_len: usize) -> ChangeSet {
        let mut ops = Vec::new();
        if at > 0 {
            ops.push(Op::Retain(at));
        }
        ops.push(Op::Insert(Tendril::from(text)));
        if at < doc_len {
            ops.push(Op::Retain(doc_len - at));
        }
        ChangeSet { ops, len_before: doc_len, len_after: doc_len + text.len() }
    }

    /// Delete `range` (bytes) in a document of length `doc_len`.
    pub fn delete(range: Range<BytePos>, doc_len: usize) -> ChangeSet {
        let mut ops = Vec::new();
        if range.start > 0 {
            ops.push(Op::Retain(range.start));
        }
        ops.push(Op::Delete(range.end - range.start));
        if range.end < doc_len {
            ops.push(Op::Retain(doc_len - range.end));
        }
        ChangeSet { ops, len_before: doc_len, len_after: doc_len - (range.end - range.start) }
    }

    /// Apply in place. O(edit size + #ops·log n): Retain only advances a cursor,
    /// so a single-key edit never copies the whole document.
    pub fn apply(&self, buf: &mut TextBuffer) {
        let mut pos: BytePos = 0;
        for op in &self.ops {
            match op {
                Op::Retain(n) => pos += n,
                Op::Delete(n) => buf.delete(pos..pos + n), // pos stays; tail shifts left
                Op::Insert(s) => {
                    buf.insert(pos, s);
                    pos += s.len();
                }
            }
        }
    }

    /// Produce the inverse changeset. Needs the *original* buffer to recover the
    /// bytes a Delete removed (re-emitted as an Insert).
    pub fn invert(&self, original: &TextBuffer) -> ChangeSet {
        let mut inv = Vec::with_capacity(self.ops.len());
        let mut pos: BytePos = 0; // position in the ORIGINAL text
        for op in &self.ops {
            match op {
                Op::Retain(n) => {
                    inv.push(Op::Retain(*n));
                    pos += n;
                }
                Op::Delete(n) => {
                    let removed = original.slice(pos..pos + n);
                    inv.push(Op::Insert(Tendril::from(removed.as_str())));
                    pos += n;
                }
                Op::Insert(s) => {
                    // inserted text is not present in the original; pos unchanged
                    inv.push(Op::Delete(s.len()));
                }
            }
        }
        ChangeSet { ops: inv, len_before: self.len_after, len_after: self.len_before }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_insert() {
        let mut b = TextBuffer::from_str("hello world");
        let cs = ChangeSet::insert(5, ",", b.len());
        cs.apply(&mut b);
        assert_eq!(b.to_string(), "hello, world");
    }

    #[test]
    fn apply_delete() {
        let mut b = TextBuffer::from_str("hello, world");
        let cs = ChangeSet::delete(5..7, b.len()); // remove ", "
        cs.apply(&mut b);
        assert_eq!(b.to_string(), "helloworld");
    }

    #[test]
    fn len_fields_track_size() {
        let b = TextBuffer::from_str("abc");
        let ins = ChangeSet::insert(1, "XY", b.len());
        assert_eq!((ins.len_before, ins.len_after), (3, 5));
        let del = ChangeSet::delete(0..2, b.len());
        assert_eq!((del.len_before, del.len_after), (3, 1));
    }

    #[test]
    fn invert_restores_original() {
        let original = TextBuffer::from_str("hello world");
        let cs = ChangeSet::delete(5..11, original.len()); // delete " world"
        let inv = cs.invert(&original);

        let mut b = original.clone();
        cs.apply(&mut b);
        assert_eq!(b.to_string(), "hello");
        inv.apply(&mut b);
        assert_eq!(b.to_string(), "hello world"); // round-trip
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        // LAW (spec §11.2): apply(invert(cs)) ∘ apply(cs) == identity.
        #[test]
        fn prop_apply_then_invert_is_identity(
            text in ".{0,40}",
            at in 0usize..40,
            ins in ".{0,8}",
            del_len in 0usize..40,
        ) {
            let original = TextBuffer::from_str(&text);
            let len = original.len();
            // clamp to valid byte boundaries by snapping onto char starts
            let at = snap(&text, at.min(len));
            // build either an insert or a bounded delete
            let cs = if del_len == 0 || at >= len {
                ChangeSet::insert(at, &ins, len)
            } else {
                let end = snap(&text, (at + del_len).min(len));
                ChangeSet::delete(at..end, len)
            };
            let inv = cs.invert(&original);
            let mut b = original.clone();
            cs.apply(&mut b);
            inv.apply(&mut b);
            prop_assert_eq!(b.to_string(), text);
        }
    }

    // test helper: snap a byte index down to the nearest char boundary
    fn snap(s: &str, mut i: usize) -> usize {
        while i < s.len() && !s.is_char_boundary(i) {
            i -= 1;
        }
        i.min(s.len())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// LAW (spec §11.2): apply(invert(cs)) ∘ apply(cs) == identity for a
        /// genuine multi-op ChangeSet (≥ 3 ops: Retain + Delete + Insert + Retain).
        /// Builds the ChangeSet by walking real char-boundary-aligned segments so
        /// all ops land correctly, guaranteeing at least one Delete AND one Insert.
        #[test]
        fn prop_multi_op_invert_is_identity(
            text in ".{4,40}",
            // fraction in [0,100) → position of the cut point (scaled to [1, len-2])
            cut_frac in 1usize..99,
            ins in "[a-z]{1,6}",
        ) {
            let doc = text.as_str();
            let len = doc.len();
            // Need at least one char before AND after the cut so Delete/Retain can span them.
            if len < 2 { return Ok(()); }

            // Pick a cut point somewhere in the middle, snapped to a char boundary.
            let raw_cut = 1 + (cut_frac * (len - 1)) / 100;
            let cut = snap(doc, raw_cut.min(len - 1));
            // If snap pushed cut to 0 or len, skip — can't form a proper multi-op.
            if cut == 0 || cut == len { return Ok(()); }

            // Build a 4+-op ChangeSet manually:
            //   Retain(cut)  Delete(len - cut)  Insert(ins)
            // len_before = len
            // len_after  = cut + ins.len()
            let mut ops = Vec::new();
            ops.push(Op::Retain(cut));
            ops.push(Op::Delete(len - cut));
            ops.push(Op::Insert(Tendril::from(ins.as_str())));
            let cs = ChangeSet {
                ops,
                len_before: len,
                len_after: cut + ins.len(),
            };

            // Sanity: at least 3 ops (Retain + Delete + Insert).
            prop_assert!(cs.ops.len() >= 3, "changeset must have ≥ 3 ops");

            let original = TextBuffer::from_str(doc);
            let inv = cs.invert(&original);

            let mut b = original.clone();
            cs.apply(&mut b);
            inv.apply(&mut b);
            prop_assert_eq!(b.to_string(), text,
                "round-trip failed: cut={} ins={:?}", cut, ins);
        }
    }
}
