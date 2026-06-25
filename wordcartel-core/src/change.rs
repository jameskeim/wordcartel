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
