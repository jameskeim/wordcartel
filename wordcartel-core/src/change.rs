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
    ///
    /// If `at > doc_len` the position is clamped to `doc_len` (safe in release;
    /// the `debug_assert` fires in debug builds to catch caller bugs early).
    pub fn insert(at: BytePos, text: &str, doc_len: usize) -> ChangeSet {
        debug_assert!(at <= doc_len, "insert at {} past doc_len {}", at, doc_len);
        let at = at.min(doc_len);
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
    ///
    /// A reversed range (`range.start > range.end`) is normalized to the
    /// equivalent forward range.  Either endpoint beyond `doc_len` is clamped.
    /// The `debug_assert` fires in debug builds to catch caller bugs early;
    /// release builds clamp silently.
    pub fn delete(range: Range<BytePos>, doc_len: usize) -> ChangeSet {
        debug_assert!(
            range.start <= range.end && range.end <= doc_len,
            "delete range {:?} invalid for doc_len {}",
            range,
            doc_len
        );
        // Normalize: handle reversed range, then clamp both endpoints.
        let start = range.start.min(range.end);
        let end = range.start.max(range.end);
        let end = end.min(doc_len);
        let start = start.min(end);
        let del_len = end - start;
        let mut ops = Vec::new();
        if start > 0 {
            ops.push(Op::Retain(start));
        }
        ops.push(Op::Delete(del_len));
        if end < doc_len {
            ops.push(Op::Retain(doc_len - end));
        }
        ChangeSet { ops, len_before: doc_len, len_after: doc_len - del_len }
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

    // ── Fix A: ChangeSet constructor clamping / normalization ──────────────────

    /// A reversed range must produce the same changeset + result as the forward range.
    /// Only runs in release mode; in debug mode the debug_assert tripwire fires first.
    #[test]
    #[cfg(not(debug_assertions))]
    fn delete_reversed_range_equals_forward() {
        let mut fwd = TextBuffer::from_str("hello world");
        let cs_fwd = ChangeSet::delete(5..7, fwd.len());

        let mut rev = TextBuffer::from_str("hello world");
        let cs_rev = ChangeSet::delete(7..5, rev.len()); // reversed

        // Both changesets must be structurally identical.
        assert_eq!(cs_fwd, cs_rev);
        cs_fwd.apply(&mut fwd);
        cs_rev.apply(&mut rev);
        assert_eq!(fwd.to_string(), rev.to_string());
    }

    /// `range.end` beyond `doc_len` clamps to `doc_len`.
    /// Only runs in release mode; in debug mode the debug_assert tripwire fires first.
    #[test]
    #[cfg(not(debug_assertions))]
    fn delete_range_end_beyond_doc_len_clamps() {
        let mut b = TextBuffer::from_str("hello");
        let len = b.len(); // 5
        let cs = ChangeSet::delete(2..99, len); // end way past the end
        cs.apply(&mut b);
        assert_eq!(b.to_string(), "he"); // deleted bytes 2..5
        assert_eq!(cs.len_after, 2);
    }

    /// `at` beyond `doc_len` clamps to `doc_len` (appends).
    /// Only runs in release mode; in debug mode the debug_assert tripwire fires first.
    #[test]
    #[cfg(not(debug_assertions))]
    fn insert_at_beyond_doc_len_clamps() {
        let mut b = TextBuffer::from_str("hello");
        let len = b.len(); // 5
        let cs = ChangeSet::insert(99, "!", len); // at way past the end
        cs.apply(&mut b);
        assert_eq!(b.to_string(), "hello!"); // appended
        assert_eq!(cs.len_after, len + 1);
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

            // Build a 3-op ChangeSet manually (Retain + Delete + Insert):
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
