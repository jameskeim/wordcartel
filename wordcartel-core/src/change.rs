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

// INVARIANT: all positions and lengths here are byte offsets into a single
// in-memory document, so they are bounded by the document's byte length and fit a
// `usize`. On the 64-bit targets Wordcartel supports, the length/position
// arithmetic below (`doc_len + text.len()`, `pos + n`, etc.) cannot overflow for
// any real document — the perf budget caps editing at ~5 MB (spec §3.9), ~12 orders
// of magnitude below `usize::MAX`. Malformed *positions* (e.g. past end-of-doc) do
// not corrupt silently: they hit `TextBuffer`'s release-enforced char-boundary
// `assert!` (buffer.rs) during `apply`. We therefore deliberately keep plain
// arithmetic on the per-keystroke hot path rather than `checked_add`.
impl ChangeSet {
    /// Insert `text` at byte offset `at` in a document of length `doc_len`.
    ///
    /// Panics (release) if `at > doc_len`.
    pub fn insert(at: BytePos, text: &str, doc_len: usize) -> ChangeSet {
        assert!(at <= doc_len, "insert at {} past doc_len {}", at, doc_len);
        let mut ops = Vec::new();
        if at > 0 { ops.push(Op::Retain(at)); }
        ops.push(Op::Insert(Tendril::from(text)));
        if at < doc_len { ops.push(Op::Retain(doc_len - at)); }
        ChangeSet { ops, len_before: doc_len, len_after: doc_len + text.len() }
    }

    /// Delete `range` (bytes) in a document of length `doc_len`.
    ///
    /// A reversed range (`range.start > range.end`) is normalized to the
    /// equivalent forward range.  Panics (release) if `end > doc_len`.
    pub fn delete(range: Range<BytePos>, doc_len: usize) -> ChangeSet {
        // Normalize a reversed range (head..anchor); then assert in-bounds.
        let start = range.start.min(range.end);
        let end = range.start.max(range.end);
        assert!(end <= doc_len, "delete end {} past doc_len {}", end, doc_len);
        let del_len = end - start;
        let mut ops = Vec::new();
        if start > 0 { ops.push(Op::Retain(start)); }
        ops.push(Op::Delete(del_len));
        if end < doc_len { ops.push(Op::Retain(doc_len - end)); }
        ChangeSet { ops, len_before: doc_len, len_after: doc_len - del_len }
    }

    /// Apply in place. O(edit size + #ops·log n): Retain only advances a cursor,
    /// so a single-key edit never copies the whole document.
    pub fn apply(&self, buf: &mut TextBuffer) {
        assert!(
            buf.len() == self.len_before,
            "apply: buf.len() {} != len_before {}",
            buf.len(), self.len_before
        );
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

    /// Build a ChangeSet from raw ops over a document of length `len_before`.
    /// Computes `len_after` from the ops; release-asserts the consumption invariant
    /// `sum(Retain)+sum(Delete) == len_before`. (UTF-8 op-boundary correctness is NOT
    /// checked here — there is no document; it stays enforced by TextBuffer's asserts in
    /// `apply`.) Trusted-caller constructor; the future plugin path validates upstream.
    pub fn from_ops(ops: Vec<Op>, len_before: usize) -> ChangeSet {
        let (mut retain, mut delete, mut insert) = (0usize, 0usize, 0usize);
        for op in &ops {
            match op {
                Op::Retain(n) => retain += n,
                Op::Delete(n) => delete += n,
                Op::Insert(s) => insert += s.len(),
            }
        }
        assert!(
            retain + delete == len_before,
            "from_ops: retain+delete {} != len_before {}",
            retain + delete, len_before
        );
        ChangeSet { ops, len_before, len_after: retain + insert }
    }

    /// Document length after this changeset applies.
    pub fn len_after(&self) -> usize { self.len_after }

    /// Document length this changeset expects before applying.
    pub fn len_before(&self) -> usize { self.len_before }

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

/// Map one byte position through a ChangeSet (insertion bias = After).
/// Shared by `Selection` mapping and 5c marks/ring mapping.
pub fn map_pos(pos: usize, cs: &ChangeSet) -> usize {
    let mut old = 0usize;
    let mut new = 0usize;
    for op in &cs.ops {
        match op {
            Op::Retain(n) => {
                if pos < old + n { return new + (pos - old); }
                old += n; new += n;
            }
            Op::Insert(s) => { new += s.len(); }
            Op::Delete(n) => {
                if pos < old + n { return new; }
                old += n;
            }
        }
    }
    new + pos.saturating_sub(old)
}

/// Map one byte position through a ChangeSet with insertion bias = Before.
/// A position sitting exactly at a PURE insertion point stays BEFORE the
/// inserted text (the opposite of `map_pos`). Used for fold anchors at heading
/// starts: inserting text at the heading's first byte must not push the anchor
/// into the body. An insertion that follows a deletion ending at the anchor (a
/// replace) behaves like `map_pos` — the anchor advances past the new text.
/// Deletion behaviour matches `map_pos` (a position inside a deletion clamps to
/// the deletion start).
pub fn map_pos_before(pos: usize, cs: &ChangeSet) -> usize {
    let mut old = 0usize;
    let mut new = 0usize;
    let mut prev_was_delete = false; // did the previous op delete up to `old`?
    for op in &cs.ops {
        match op {
            Op::Retain(n) => {
                if pos < old + n { return new + (pos - old); }
                old += n; new += n;
                prev_was_delete = false;
            }
            Op::Insert(s) => {
                // Before bias for a PURE insertion at the anchor only. After a
                // delete-to-here (replace), fall through so `pos` advances past
                // the inserted text (matches map_pos).
                if pos == old && !prev_was_delete { return new; }
                new += s.len();
                prev_was_delete = false;
            }
            Op::Delete(n) => {
                if pos < old + n { return new; }
                old += n;
                prev_was_delete = true;
            }
        }
    }
    new + pos.saturating_sub(old)
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

    /// `range.end` beyond `doc_len` now panics (fail-fast, no clamping).
    #[test]
    #[should_panic(expected = "doc_len")]
    fn delete_range_end_beyond_doc_len_clamps() {
        let len = TextBuffer::from_str("hello").len(); // 5
        let _ = ChangeSet::delete(2..99, len); // end way past the end → panic
    }

    /// `at` beyond `doc_len` now panics (fail-fast, no clamping).
    #[test]
    #[should_panic(expected = "doc_len")]
    fn insert_at_beyond_doc_len_clamps() {
        let len = TextBuffer::from_str("hello").len(); // 5
        let _ = ChangeSet::insert(99, "!", len); // at way past the end → panic
    }

    // ── Task 1 (M1): new failing tests ────────────────────────────────────────

    #[test]
    fn from_ops_computes_len_after_and_accepts_valid() {
        use super::*;
        // doc "abc" (3), delete "b", insert "XY": Retain(1) Delete(1) Insert("XY") Retain(1)
        let ops = vec![Op::Retain(1), Op::Delete(1), Op::Insert(Tendril::from("XY")), Op::Retain(1)];
        let cs = ChangeSet::from_ops(ops, 3);
        assert_eq!(cs.len_before(), 3);
        assert_eq!(cs.len_after(), 4); // retain 2 + insert 2
    }

    #[test]
    #[should_panic(expected = "len_before")]
    fn from_ops_rejects_non_summing_ops() {
        use super::*;
        // Retain(1)+Delete(1) = 2, but len_before claimed 5.
        let _ = ChangeSet::from_ops(vec![Op::Retain(1), Op::Delete(1), Op::Retain(1)], 5);
    }

    #[test]
    #[should_panic(expected = "len_before")]
    fn apply_rejects_buffer_length_mismatch() {
        use super::*;
        let cs = ChangeSet::insert(0, "x", 3); // built for doc_len 3
        let mut buf = TextBuffer::from_str("ab"); // len 2 ≠ 3
        cs.apply(&mut buf);
    }

    #[test]
    #[should_panic(expected = "doc_len")]
    fn insert_panics_past_doc_len() {
        use super::*;
        let _ = ChangeSet::insert(10, "x", 3);
    }

    #[test]
    #[should_panic(expected = "doc_len")]
    fn delete_panics_out_of_bounds() {
        use super::*;
        let _ = ChangeSet::delete(2..10, 3);
    }

    #[test]
    fn delete_normalizes_reversed_in_bounds_range() {
        use super::*;
        // reversed but in-bounds: 3..1 on doc_len 5 → deletes [1,3)
        let cs = ChangeSet::delete(3..1, 5);
        assert_eq!(cs.len_before(), 5);
        assert_eq!(cs.len_after(), 3); // deleted 2 bytes
    }

    #[test]
    fn map_pos_shifts_after_insert_and_clamps_in_delete() {
        use crate::buffer::TextBuffer;
        let buf = TextBuffer::from_str("abcdef");
        // insert "XY" at offset 2 → positions >= 2 shift by 2
        let cs = ChangeSet::insert(2, "XY", buf.len());
        assert_eq!(map_pos(0, &cs), 0);
        assert_eq!(map_pos(2, &cs), 4); // bias After
        assert_eq!(map_pos(5, &cs), 7);
        // delete 2..4 → a position inside the deletion clamps to its start
        let cs2 = ChangeSet::delete(2..4, buf.len());
        assert_eq!(map_pos(3, &cs2), 2);
        assert_eq!(map_pos(5, &cs2), 3);
    }

    #[test]
    fn map_pos_before_keeps_anchor_before_insertion() {
        use crate::buffer::TextBuffer;
        let buf = TextBuffer::from_str("abcdef");
        // insert "XY" at offset 2
        let cs = ChangeSet::insert(2, "XY", buf.len());
        // After-biased map_pos moves 2 -> 4; the Before variant keeps it at 2.
        assert_eq!(map_pos(2, &cs), 4);
        assert_eq!(map_pos_before(2, &cs), 2);
        // positions strictly after the insertion still shift by the insert length
        assert_eq!(map_pos_before(3, &cs), 5);
        // positions strictly before are unchanged
        assert_eq!(map_pos_before(1, &cs), 1);
        // insertion at offset 0 keeps a byte-0 anchor at 0
        let cs0 = ChangeSet::insert(0, "Z", buf.len());
        assert_eq!(map_pos_before(0, &cs0), 0);
        // deletion behaves identically to map_pos (clamp to deletion start)
        let csd = ChangeSet::delete(2..4, buf.len());
        assert_eq!(map_pos_before(3, &csd), 2);
        assert_eq!(map_pos_before(5, &csd), 3);
        // REPLACE 2..4 with "XY": an anchor at byte 4 (right edge of the replace =
        // the next heading start) must map AFTER the new text (4), NOT back onto it.
        // Build the real Retain,Delete,Insert,Retain shape the shell emits.
        let cs_rep = ChangeSet {
            ops: vec![Op::Retain(2), Op::Delete(2), Op::Insert("XY".into()), Op::Retain(2)],
            len_before: buf.len(),
            len_after: buf.len(),
        };
        assert_eq!(map_pos_before(4, &cs_rep), 4); // not 2
        // a PURE insert at a mid-doc boundary still stays before
        let cs_mid = ChangeSet::insert(4, "Q", buf.len());
        assert_eq!(map_pos_before(4, &cs_mid), 4);
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

    /// Alphabet that includes multi-byte graphemes: ASCII letters, accented chars,
    /// CJK, emoji, and a combining sequence.
    fn multibyte_alphabet() -> impl Strategy<Value = String> {
        let chars = prop::collection::vec(
            prop::sample::select(vec![
                'a', 'b', 'c', 'd', 'e',
                'é',            // U+00E9  (2 bytes)
                '中',           // U+4E2D  (3 bytes)
                '🙂',           // U+1F642 (4 bytes)
            ]),
            0..=20,
        );
        chars.prop_map(|v| v.into_iter().collect::<String>())
    }

    /// Short multibyte string for inserted text (always char-aligned by construction).
    fn multibyte_ins() -> impl Strategy<Value = String> {
        let chars = prop::collection::vec(
            prop::sample::select(vec![
                'x', 'y', 'z',
                'é',
                '中',
                '🙂',
            ]),
            1..=6,
        );
        chars.prop_map(|v| v.into_iter().collect::<String>())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// LAW (spec §11.2): apply(invert(cs)) ∘ apply(cs) == identity for a
        /// genuine 4-op ChangeSet: Retain(c1) Delete(d) Insert(ins) Retain(c2).
        /// Uses a multi-byte alphabet (ASCII + é + 中 + 🙂) so all segments can
        /// include CJK, emoji, and accented characters.  Two cut points divide the
        /// document: the first starts the Delete span, the second ends it (leaving
        /// a trailing Retain), exercising the full 4-op shape.
        #[test]
        fn prop_multi_op_invert_is_identity(
            text in multibyte_alphabet().prop_filter("need ≥2 chars", |s| s.chars().count() >= 2),
            // Two fractions select c1 and c1+d boundaries inside the document.
            c1_frac in 1usize..50,
            d_frac  in 51usize..99,
            ins in multibyte_ins(),
        ) {
            let doc = text.as_str();
            let len = doc.len();
            if len < 2 { return Ok(()); }

            // Derive c1 (start of Delete) and c2_start (end of Delete) as char-boundary
            // byte offsets, guaranteed c1 < c2_start < len.
            let raw_c1 = 1 + (c1_frac * (len - 1)) / 100;
            let c1 = snap(doc, raw_c1.min(len - 1));

            let raw_c2_start = 1 + (d_frac * (len - 1)) / 100;
            let c2_start = snap(doc, raw_c2_start.min(len - 1));

            // Need c1 < c2_start with at least one char retained at the tail.
            if c1 == 0 || c2_start <= c1 || c2_start == len { return Ok(()); }

            let d = c2_start - c1;    // bytes deleted
            let c2 = len - c2_start;  // bytes in trailing Retain

            // Shape: Retain(c1)  Delete(d)  Insert(ins)  Retain(c2)
            // len_before = c1 + d + c2 = len
            // len_after  = c1 + ins.len() + c2
            let mut ops = Vec::new();
            ops.push(Op::Retain(c1));
            ops.push(Op::Delete(d));
            ops.push(Op::Insert(Tendril::from(ins.as_str())));
            ops.push(Op::Retain(c2));
            let cs = ChangeSet {
                ops,
                len_before: len,
                len_after: c1 + ins.len() + c2,
            };

            // Sanity: 4-op shape.
            prop_assert_eq!(cs.ops.len(), 4, "expected 4 ops; got {:?}", cs.ops);

            let original = TextBuffer::from_str(doc);
            let inv = cs.invert(&original);

            let mut b = original.clone();
            cs.apply(&mut b);
            inv.apply(&mut b);
            prop_assert_eq!(b.to_string(), text,
                "round-trip failed: c1={} d={} ins={:?} c2={}", c1, d, ins, c2);
        }
    }
}
