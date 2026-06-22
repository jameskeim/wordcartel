//! In-process clipboard register. Copy/cut/paste always work via this register,
//! independent of any system clipboard (spec §9.5/§15.6).
use crate::buffer::TextBuffer;
use crate::change::ChangeSet;
use crate::selection::Range;
use crate::BytePos;

#[derive(Clone, Debug, Default)]
pub struct Register {
    pub text: Option<String>,
}

impl Register {
    pub fn set(&mut self, text: String) {
        self.text = Some(text);
    }
    pub fn get(&self) -> Option<&str> {
        self.text.as_deref()
    }
}

pub fn copy(buf: &TextBuffer, range: Range, reg: &mut Register) {
    reg.set(buf.slice(range.from()..range.to()));
}

pub fn cut(range: Range, doc_len: usize, reg: &mut Register, buf: &TextBuffer) -> ChangeSet {
    reg.set(buf.slice(range.from()..range.to()));
    ChangeSet::delete(range.from()..range.to(), doc_len)
}

pub fn paste(at: BytePos, doc_len: usize, reg: &Register) -> Option<ChangeSet> {
    reg.get().map(|t| ChangeSet::insert(at, t, doc_len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_then_paste() {
        let buf = TextBuffer::from_str("hello world");
        let mut reg = Register::default();
        copy(&buf, Range { anchor: 0, head: 5 }, &mut reg); // "hello"
        assert_eq!(reg.text.as_deref(), Some("hello"));

        let cs = paste(11, buf.len(), &reg).unwrap();
        let mut b2 = buf.clone();
        cs.apply(&mut b2);
        assert_eq!(b2.to_string(), "hello worldhello");
    }

    #[test]
    fn cut_removes_and_fills_register() {
        let buf = TextBuffer::from_str("hello world");
        let mut reg = Register::default();
        let cs = cut(Range { anchor: 5, head: 11 }, buf.len(), &mut reg, &buf); // " world"
        assert_eq!(reg.text.as_deref(), Some(" world"));
        let mut b = buf.clone();
        cs.apply(&mut b);
        assert_eq!(b.to_string(), "hello");
    }

    #[test]
    fn paste_empty_register_is_none() {
        let reg = Register::default();
        assert!(paste(0, 0, &reg).is_none());
    }
}
