//! TextBuffer: the only owner of byte↔char↔line conversion (spec §16.1).
use crate::BytePos;
use std::ops::Range;

#[derive(Clone, Debug)]
pub struct TextBuffer {
    rope: ropey::Rope,
}

impl TextBuffer {
    pub fn from_str(s: &str) -> Self {
        TextBuffer { rope: ropey::Rope::from_str(s) }
    }

    pub fn len(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }

    /// Returns `true` iff `b` is a valid UTF-8 char boundary in the rope.
    /// 0 and `self.len()` are always boundaries (empty or at the seam).
    fn is_char_boundary(&self, b: BytePos) -> bool {
        b == 0 || b == self.len() || self.rope.char_to_byte(self.rope.byte_to_char(b)) == b
    }

    pub fn insert(&mut self, at: BytePos, text: &str) {
        assert!(
            self.is_char_boundary(at),
            "insert at non-char-boundary byte {at}: would corrupt the buffer"
        );
        let char_idx = self.rope.byte_to_char(at);
        self.rope.insert(char_idx, text);
    }

    pub fn delete(&mut self, range: Range<BytePos>) {
        assert!(
            self.is_char_boundary(range.start),
            "delete range.start ({}) is not a char-boundary: would corrupt the buffer",
            range.start
        );
        assert!(
            self.is_char_boundary(range.end),
            "delete range.end ({}) is not a char-boundary: would corrupt the buffer",
            range.end
        );
        let start = self.rope.byte_to_char(range.start);
        let end = self.rope.byte_to_char(range.end);
        self.rope.remove(start..end);
    }

    pub fn slice(&self, range: Range<BytePos>) -> String {
        assert!(
            self.is_char_boundary(range.start),
            "slice range.start ({}) is not a char-boundary: would corrupt the buffer",
            range.start
        );
        assert!(
            self.is_char_boundary(range.end),
            "slice range.end ({}) is not a char-boundary: would corrupt the buffer",
            range.end
        );
        self.rope.byte_slice(range).to_string()
    }

    pub fn byte_to_line(&self, b: BytePos) -> usize {
        self.rope.byte_to_line(b)
    }

    pub fn line_to_byte(&self, line: usize) -> BytePos {
        self.rope.line_to_byte(line)
    }

    pub fn snapshot(&self) -> ropey::Rope {
        self.rope.clone() // O(1) — the async-worker seam (spec §10.3)
    }

    pub fn to_string(&self) -> String {
        self.rope.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_delete_ascii() {
        let mut b = TextBuffer::from_str("hello world");
        b.insert(5, ",");
        assert_eq!(b.to_string(), "hello, world");
        b.delete(0..7); // remove "hello, "
        assert_eq!(b.to_string(), "world");
        assert_eq!(b.len(), 5);
    }

    #[test]
    fn slice_and_multibyte() {
        // "héllo" — 'é' is 2 bytes (U+00E9). bytes: h(0) é(1..3) l(3) l(4) o(5)
        let b = TextBuffer::from_str("héllo");
        assert_eq!(b.len(), 6);
        assert_eq!(b.slice(0..3), "hé");
        assert_eq!(b.slice(3..6), "llo");
    }

    #[test]
    fn line_conversions() {
        let b = TextBuffer::from_str("a\nbb\nccc");
        // bytes: a(0) \n(1) b(2) b(3) \n(4) c(5) c(6) c(7)
        assert_eq!(b.byte_to_line(0), 0);
        assert_eq!(b.byte_to_line(2), 1);
        assert_eq!(b.byte_to_line(5), 2);
        assert_eq!(b.line_to_byte(1), 2);
        assert_eq!(b.line_to_byte(2), 5);
    }

    #[test]
    fn snapshot_is_independent() {
        let mut b = TextBuffer::from_str("abc");
        let snap = b.snapshot();
        b.insert(3, "d");
        assert_eq!(b.to_string(), "abcd");
        assert_eq!(snap.to_string(), "abc"); // snapshot unaffected
    }

    #[test]
    fn is_char_boundary_rejects_mid_char_byte() {
        // "é" is U+00E9, encoded as [0xC3, 0xA9] — 2 bytes.
        // byte 0: 'h' boundary ✓, byte 1: first byte of é (boundary), byte 2: second
        // byte of é (NOT a boundary), byte 3: 'l' boundary ✓
        let b = TextBuffer::from_str("héllo");
        assert!(b.is_char_boundary(0), "byte 0 must be a boundary");
        assert!(b.is_char_boundary(1), "byte 1 (start of é) must be a boundary");
        assert!(!b.is_char_boundary(2), "byte 2 (inside é) must NOT be a boundary");
        assert!(b.is_char_boundary(3), "byte 3 (start of l) must be a boundary");
        assert!(b.is_char_boundary(b.len()), "len() must be a boundary");
    }

    /// Inserting at a mid-char byte offset must panic in all build profiles
    /// (release-enforced char-boundary guard).  "héllo": byte 2 is the second
    /// byte of 'é' (U+00E9 = [0xC3, 0xA9]) — not a char boundary.
    #[test]
    #[should_panic(expected = "char-boundary")]
    fn insert_at_mid_char_byte_panics() {
        let mut b = TextBuffer::from_str("héllo");
        b.insert(2, "X"); // byte 2 is inside 'é' — must panic
    }
}
