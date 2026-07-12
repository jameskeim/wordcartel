//! ChangeSet: reversible byte-diff. Reimplemented from the Helix/CodeMirror
//! transaction pattern (MPL/MIT) — pattern, not copied source (spec §9.6).
use crate::buffer::TextBuffer;
use crate::BytePos;
use std::ops::Range;

/// Small-string-optimized owned text used for `Op::Insert` payloads — a `String`
/// alias that avoids a heap allocation for short inserts (the common per-keystroke case).
pub type Tendril = smartstring::alias::String;

/// One step of a [`ChangeSet`]: retain, delete, or insert a span of bytes.
/// A `ChangeSet`'s ops consume the OLD document in order (`Retain`/`Delete` byte
/// counts sum to `len_before`) while producing the NEW document (`Retain`/`Insert`
/// bytes sum to `len_after`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Copy forward the next `usize` bytes of the old document unchanged.
    Retain(usize), // bytes
    /// Skip (remove) the next `usize` bytes of the old document.
    Delete(usize), // bytes
    /// Insert this text, not present in the old document, into the new document.
    Insert(Tendril),
}

/// A reversible byte-diff between two document states: a sequence of [`Op`]s that
/// consumes a document of `len_before` bytes and produces one of `len_after`
/// bytes. This is Wordcartel's edit-transaction type — see the module docs for
/// provenance and the invariant-arithmetic note below for the overflow argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeSet {
    ops: Vec<Op>,
    len_before: usize,
    len_after: usize,
}

/// Why an untrusted changeset was rejected against a specific buffer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditError {
    /// The changeset was built for a different document length than the buffer has.
    StaleLength {
        /// The document length (bytes) the changeset was built for (`len_before`).
        expected: usize,
        /// The buffer's actual current length (bytes).
        actual: usize,
    },
    /// An op's byte boundary lands inside a multibyte char in the buffer.
    OpBoundary {
        /// The offending byte offset — not a char boundary in the buffer.
        pos: usize,
    },
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

    /// Heap text this changeset holds: the sum of `Insert` payload byte lengths. `Retain`/`Delete`
    /// are counts (negligible structural overhead, excluded). A revision's true memory is captured
    /// by summing this over both its `changes` and `inverse` (a delete's text lives in the inverse).
    pub fn stored_bytes(&self) -> usize {
        self.ops.iter().map(|op| match op {
            Op::Insert(s) => s.len(),
            Op::Retain(_) | Op::Delete(_) => 0,
        }).sum()
    }

    /// Validate this changeset against `buf` WITHOUT mutating. Length must match
    /// (→ `StaleLength`), and every op's OLD-text byte boundaries must be char
    /// boundaries in `buf` (→ `OpBoundary`) — so a later `apply` cannot panic partway.
    /// Returns the FIRST violation; `Ok(())` means `apply(buf)` is panic-safe.
    ///
    /// Unlike `from_ops` (the per-keystroke hot path), this function is the
    /// untrusted-edit boundary and ENFORCES position bounds: every `old_pos`/`end`
    /// value passed to `is_char_boundary` is guaranteed `<= len` via checked
    /// arithmetic. An op whose cumulative position overflows or exceeds `len` is
    /// rejected as `OpBoundary` — an out-of-range position is, trivially, not a
    /// valid char boundary. `from_ops` is NOT the right place for this check
    /// because it runs on the per-keystroke hot path from trusted callers whose
    /// positions are bounded by ~5 MB (spec §3.9).
    pub fn validate_against(&self, buf: &TextBuffer) -> Result<(), EditError> {
        let len = buf.len();
        if self.len_before != len {
            return Err(EditError::StaleLength { expected: self.len_before, actual: len });
        }
        let mut old_pos: usize = 0;
        for op in &self.ops {
            match op {
                Op::Retain(n) => {
                    // Reject overflow OR past-end BEFORE any is_char_boundary call.
                    // Malformed plugin ChangeSets (e.g. Retain(usize::MAX)) whose
                    // cumulative position wraps past `len` in release builds are caught
                    // here at the untrusted boundary, not in from_ops (hot path).
                    old_pos = match old_pos.checked_add(*n) {
                        Some(p) if p <= len => p,
                        _ => return Err(EditError::OpBoundary { pos: old_pos.saturating_add(*n) }),
                    };
                }
                Op::Delete(n) => {
                    if !buf.is_char_boundary(old_pos) { return Err(EditError::OpBoundary { pos: old_pos }); }
                    let end = match old_pos.checked_add(*n) {
                        Some(p) if p <= len => p,
                        _ => return Err(EditError::OpBoundary { pos: old_pos.saturating_add(*n) }),
                    };
                    if !buf.is_char_boundary(end) { return Err(EditError::OpBoundary { pos: end }); }
                    old_pos = end;
                }
                Op::Insert(_) => {
                    if !buf.is_char_boundary(old_pos) { return Err(EditError::OpBoundary { pos: old_pos }); }
                    // old_pos unchanged: insert adds to the NEW text, not the OLD.
                }
            }
        }
        Ok(())
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

/// Test-only bypass: create a ChangeSet without the sum-invariant check.
/// Allows building adversarial ChangeSets (e.g. ops that overflow or exceed
/// len_before) to verify validate_against's out-of-range defences.
#[cfg(test)]
impl ChangeSet {
    pub(crate) fn from_ops_unchecked(ops: Vec<Op>, len_before: usize, len_after: usize) -> Self {
        ChangeSet { ops, len_before, len_after }
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
        assert_eq!((ins.len_before(), ins.len_after()), (3, 5));
        let del = ChangeSet::delete(0..2, b.len());
        assert_eq!((del.len_before(), del.len_after()), (3, 1));
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
    #[test]
    fn delete_reversed_range_equals_forward() {
        let mut fwd = TextBuffer::from_str("hello world");
        let cs_fwd = ChangeSet::delete(5..7, fwd.len());

        let mut rev = TextBuffer::from_str("hello world");
        // Intentional reversed range: exercises ChangeSet::delete's reversed-range
        // normalization. Do not "fix" the direction.
        #[allow(clippy::reversed_empty_ranges)]
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

    // ── Task 2 (M5): stored_bytes ─────────────────────────────────────────────

    #[test]
    fn stored_bytes_counts_insert_payload_only() {
        // retain 3, insert "hello" (5), delete 2 -> stored = 5 (only the Insert text).
        let cs = ChangeSet::from_ops(vec![Op::Retain(3), Op::Insert("hello".into()), Op::Delete(2)], 5);
        assert_eq!(cs.stored_bytes(), 5);
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
        // Intentional reversed range: exercises ChangeSet::delete's reversed-range
        // normalization. Do not "fix" the direction.
        #[allow(clippy::reversed_empty_ranges)]
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
        let cs_rep = ChangeSet::from_ops(
            vec![Op::Retain(2), Op::Delete(2), Op::Insert("XY".into()), Op::Retain(2)],
            buf.len(),
        );
        assert_eq!(map_pos_before(4, &cs_rep), 4); // not 2
        // a PURE insert at a mid-doc boundary still stays before
        let cs_mid = ChangeSet::insert(4, "Q", buf.len());
        assert_eq!(map_pos_before(4, &cs_mid), 4);
    }

    // ── Task 1 (M2): validate_against tests ──────────────────────────────────

    #[test]
    fn validate_against_ok_for_matching_valid_changeset() {
        use super::*;
        let buf = TextBuffer::from_str("hello"); // len 5
        let cs = ChangeSet::insert(2, "X", 5);    // built for len 5, boundary 2 is valid
        assert!(cs.validate_against(&buf).is_ok());
    }

    #[test]
    fn validate_against_stale_length() {
        use super::*;
        let buf = TextBuffer::from_str("hello"); // len 5
        let cs = ChangeSet::insert(0, "X", 3);    // built for len 3 ≠ 5
        assert_eq!(cs.validate_against(&buf), Err(EditError::StaleLength { expected: 3, actual: 5 }));
    }

    #[test]
    fn validate_against_delete_end_mid_char() {
        use super::*;
        // doc "é" = 2 bytes (0xC3 0xA9); a Delete(1) ends at byte 1 (mid-char).
        let buf = TextBuffer::from_str("é"); // len 2
        let cs = ChangeSet::from_ops(vec![Op::Delete(1), Op::Retain(1)], 2); // sum-valid
        assert_eq!(cs.validate_against(&buf), Err(EditError::OpBoundary { pos: 1 }));
    }

    #[test]
    fn validate_against_insert_mid_char() {
        use super::*;
        let buf = TextBuffer::from_str("é"); // len 2
        // Retain(1) lands old_pos at byte 1 (mid-é), then Insert there.
        let cs = ChangeSet::from_ops(vec![Op::Retain(1), Op::Insert(Tendril::from("x")), Op::Retain(1)], 2);
        assert_eq!(cs.validate_against(&buf), Err(EditError::OpBoundary { pos: 1 }));
    }

    /// M2 gate §11.2 — overflow/out-of-range defence.
    ///
    /// Before the fix, Retain(n) added `n` to `old_pos` unchecked, then the next
    /// Insert/Delete called `buf.is_char_boundary(old_pos)` with an out-of-range
    /// index → ropey's `byte_to_char` panics. After the fix, checked arithmetic
    /// in `validate_against` catches the violation and returns `Err(OpBoundary)`
    /// without touching ropey.
    ///
    /// `from_ops_unchecked` bypasses the sum-invariant assert so we can build
    /// adversarial ops in debug builds (where `from_ops`'s overflow would itself
    /// panic before we could even test the boundary).
    #[test]
    fn validate_against_rejects_overflowing_ops() {
        let buf = TextBuffer::from_str("hello"); // len 5

        // Case A: position exceeds len (Retain(6) on a len-5 buffer).
        // Old code: old_pos=6, then is_char_boundary(6) panics (6 > len_bytes=5).
        // Fixed code: 6 > len (5) → Err(OpBoundary { pos: 6 }), no ropey call.
        let cs_a = ChangeSet::from_ops_unchecked(
            vec![Op::Retain(6), Op::Insert(Tendril::from("X"))],
            5, // len_before matches buf → passes the length check
            6,
        );
        assert_eq!(cs_a.validate_against(&buf), Err(EditError::OpBoundary { pos: 6 }));

        // Case B: genuine arithmetic overflow (Retain(usize::MAX)).
        // Old code in release: wraps to a garbage old_pos, then is_char_boundary panics.
        // Fixed code: checked_add overflows → None → Err(OpBoundary), no ropey call.
        let cs_b = ChangeSet::from_ops_unchecked(
            vec![Op::Retain(usize::MAX), Op::Insert(Tendril::from("X"))],
            5,
            6,
        );
        assert!(matches!(cs_b.validate_against(&buf), Err(EditError::OpBoundary { .. })));

        // Case C: Delete whose end would overflow.
        let cs_c = ChangeSet::from_ops_unchecked(
            vec![Op::Delete(usize::MAX)],
            5,
            6,
        );
        assert!(matches!(cs_c.validate_against(&buf), Err(EditError::OpBoundary { .. })));
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
            let at = crate::test_support::snap(&text, at.min(len));
            // build either an insert or a bounded delete
            let cs = if del_len == 0 || at >= len {
                ChangeSet::insert(at, &ins, len)
            } else {
                let end = crate::test_support::snap(&text, (at + del_len).min(len));
                ChangeSet::delete(at..end, len)
            };
            let inv = cs.invert(&original);
            let mut b = original.clone();
            cs.apply(&mut b);
            inv.apply(&mut b);
            prop_assert_eq!(b.to_string(), text);
        }
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
            let c1 = crate::test_support::snap(doc, raw_c1.min(len - 1));

            let raw_c2_start = 1 + (d_frac * (len - 1)) / 100;
            let c2_start = crate::test_support::snap(doc, raw_c2_start.min(len - 1));

            // Need c1 < c2_start with at least one char retained at the tail.
            if c1 == 0 || c2_start <= c1 || c2_start == len { return Ok(()); }

            let d = c2_start - c1;    // bytes deleted
            let c2 = len - c2_start;  // bytes in trailing Retain

            // Shape: Retain(c1)  Delete(d)  Insert(ins)  Retain(c2)
            // len_before = c1 + d + c2 = len
            // len_after  = c1 + ins.len() + c2
            let ops = vec![
                Op::Retain(c1),
                Op::Delete(d),
                Op::Insert(Tendril::from(ins.as_str())),
                Op::Retain(c2),
            ];
            let cs = ChangeSet::from_ops(ops, len);

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

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        /// T2 — `apply` == naive String splice, and `invert` round-trips.
        ///
        /// For every (text, p, dl, ins): build a real replace ChangeSet (Retain/Delete/Insert/Retain),
        /// verify that `apply` yields the same bytes as `replace_range(at..end, ins)` on a String,
        /// then verify `invert` restores the original. Covers empty doc, replace-at-0, replace-at-len,
        /// and multibyte/emoji char boundaries (via `prop_unicode_string`).
        /// NOTE: `invert` is computed BEFORE `apply` — it captures the about-to-be-deleted bytes.
        #[test]
        fn t2_apply_equals_string_splice(
            text in crate::proptest_strategies::prop_unicode_string(),
            p in 0usize..60, dl in 0usize..20,
            ins in crate::proptest_strategies::prop_unicode_string(),
        ) {
            let len = text.len();
            let at  = crate::test_support::snap(&text, p.min(len));
            let end = crate::test_support::snap(&text, (at + dl).min(len));
            // Omit zero-length ops — from_ops validates retain+delete == len_before.
            let mut ops = Vec::new();
            if at > 0          { ops.push(Op::Retain(at)); }
            if end > at        { ops.push(Op::Delete(end - at)); }
            if !ins.is_empty() { ops.push(Op::Insert(ins.as_str().into())); }
            if end < len       { ops.push(Op::Retain(len - end)); }
            let cs       = ChangeSet::from_ops(ops, len);
            let original = TextBuffer::from_str(&text);
            let inv      = cs.invert(&original);         // invert BEFORE apply
            let mut buf  = TextBuffer::from_str(&text);
            cs.apply(&mut buf);
            let mut model = text.clone();
            crate::test_support::model_apply(&mut model, at, end - at, &ins);
            prop_assert_eq!(buf.slice(0..buf.len()), model); // apply == naive splice
            inv.apply(&mut buf);
            prop_assert_eq!(buf.slice(0..buf.len()), text);  // invert round-trips
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        /// T3 — `map_pos`/`map_pos_before` are on-boundary, monotonic, and correctly biased.
        ///
        /// For a pure insert ChangeSet: both functions produce output positions that land on char
        /// boundaries in the new doc; both are monotone (p1 ≤ p2 → f(p1) ≤ f(p2)); and at the
        /// insertion point the before-bias stays left while the after-bias lands right
        /// (`map_pos_before(at) ≤ map_pos(at)`).
        #[test]
        fn t3_map_pos_boundary_monotonic_bias(
            text in crate::proptest_strategies::prop_unicode_string(),
            at in 0usize..60, ins in crate::proptest_strategies::prop_unicode_string(),
            q1 in 0usize..60, q2 in 0usize..60,
        ) {
            let len = text.len();
            let at  = crate::test_support::snap(&text, at.min(len));
            let cs  = ChangeSet::insert(at, &ins, len);
            let new_text = {
                let mut m = text.clone();
                crate::test_support::model_apply(&mut m, at, 0, &ins);
                m
            };
            // Derive two sorted char-boundary positions in the old doc.
            let (p1, p2) = (
                crate::test_support::snap(&text, q1.min(len))
                    .min(crate::test_support::snap(&text, q2.min(len))),
                crate::test_support::snap(&text, q1.min(len))
                    .max(crate::test_support::snap(&text, q2.min(len))),
            );
            // on-boundary: outputs land on char boundaries of the new doc
            prop_assert!(new_text.is_char_boundary(map_pos(p1, &cs)));
            prop_assert!(new_text.is_char_boundary(map_pos_before(p1, &cs)));
            // monotonic
            prop_assert!(map_pos(p1, &cs) <= map_pos(p2, &cs));
            prop_assert!(map_pos_before(p1, &cs) <= map_pos_before(p2, &cs));
            // bias at the insertion point: before stays left, after lands right
            prop_assert!(map_pos_before(at, &cs) <= map_pos(at, &cs));
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2048))]

        /// The load-bearing guarantee: if `validate_against` returns Ok, then `apply`
        /// NEVER panics and yields exactly `len_after`. (If Err, nothing is applied.)
        #[test]
        fn validated_changeset_applies_without_panic(
            doc in proptest::collection::vec(
                proptest::sample::select(vec!['a', 'é', '中', '🙂', '\n']), 0..24usize
            ).prop_map(|cs| cs.into_iter().collect::<String>()),
            claimed_len in 0usize..28,
            is_delete in proptest::bool::ANY,
            p1 in 0usize..28,
            p2 in 0usize..28,
            text in proptest::string::string_regex("[aé中]{0,4}").unwrap(),
        ) {
            use super::*;
            let buf = TextBuffer::from_str(&doc);
            // Build a SUM-VALID changeset (valid-by-construction via M1's insert/delete)
            // for `claimed_len` — which may NOT match the buffer (→ StaleLength) and whose
            // positions may land mid-char in the buffer (→ OpBoundary). insert/delete assert
            // their offset ≤ claimed_len, so keep positions in range of claimed_len.
            let cs = if is_delete {
                let a = p1 % (claimed_len + 1);
                let b = p2 % (claimed_len + 1);
                ChangeSet::delete(a.min(b)..a.max(b), claimed_len)
            } else {
                let at = p1 % (claimed_len + 1);
                ChangeSet::insert(at, &text, claimed_len)
            };
            match cs.validate_against(&buf) {
                Ok(()) => {
                    let mut b = buf.clone();
                    cs.apply(&mut b);                       // must NOT panic
                    prop_assert_eq!(b.len(), cs.len_after());
                }
                Err(_) => { /* rejected — buffer never touched */ }
            }
        }
    }
}
