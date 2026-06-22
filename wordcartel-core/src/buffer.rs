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

    pub fn insert(&mut self, at: BytePos, text: &str) {
        let char_idx = self.rope.byte_to_char(at);
        self.rope.insert(char_idx, text);
    }

    pub fn delete(&mut self, range: Range<BytePos>) {
        let start = self.rope.byte_to_char(range.start);
        let end = self.rope.byte_to_char(range.end);
        self.rope.remove(start..end);
    }

    pub fn slice(&self, range: Range<BytePos>) -> String {
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
}
