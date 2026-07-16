//! In-process clipboard register. Copy/cut/paste always work via this register,
//! independent of any system clipboard (spec §9.5/§15.6).
use crate::buffer::TextBuffer;
use crate::change::ChangeSet;
use crate::selection::Range;
use crate::BytePos;

/// An in-process clipboard slot. Holds at most one piece of text; `copy`/`cut`
/// overwrite it, `paste` reads it without consuming it. `None` means the
/// register has never been filled (or was constructed via [`Register::default`]).
#[derive(Clone, Debug, Default)]
pub struct Register {
    /// The register's current contents, or `None` if it has never been filled.
    pub text: Option<String>,
}

impl Register {
    /// Overwrite the register's contents with `text`.
    pub fn set(&mut self, text: String) {
        self.text = Some(text);
    }
    /// Borrow the register's contents, or `None` if it is empty.
    pub fn get(&self) -> Option<&str> {
        self.text.as_deref()
    }
}

/// Copy the text spanned by `range` from `buf` into `reg`, leaving `buf` unmodified.
pub fn copy(buf: &TextBuffer, range: Range, reg: &mut Register) {
    reg.set(buf.slice(range.from()..range.to()));
}

/// Build a [`ChangeSet`] that inserts the register's contents at byte offset `at`
/// in a document of length `doc_len`. Returns `None` if the register is empty
/// (nothing to paste); the caller applies the returned changeset to insert the text.
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
    fn paste_empty_register_is_none() {
        let reg = Register::default();
        assert!(paste(0, 0, &reg).is_none());
    }

    /// A REVERSE selection (anchor > head) should copy the same text as the
    /// forward equivalent. Range::from()/to() normalise the order.
    #[test]
    fn copy_reverse_selection_matches_forward() {
        let buf = TextBuffer::from_str("hello world");
        let mut reg_fwd = Register::default();
        let mut reg_rev = Register::default();

        // Forward: anchor=0, head=5 → "hello"
        copy(&buf, Range { anchor: 0, head: 5 }, &mut reg_fwd);
        // Reverse: anchor=5, head=0 → should also be "hello"
        copy(&buf, Range { anchor: 5, head: 0 }, &mut reg_rev);

        assert_eq!(reg_fwd.get(), Some("hello"));
        assert_eq!(reg_rev.get(), reg_fwd.get(),
            "reverse selection should copy identical text to forward selection");
    }
}
