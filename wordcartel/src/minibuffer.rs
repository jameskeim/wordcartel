/// Single-line input widget for command prompts (e.g. the `filter` command).
///
/// The cursor is a *byte* offset into `text`, advanced/retreated by whole
/// UTF-8 codepoints.  Multibyte caret arithmetic is safe because `text` is
/// typically very short (a shell invocation).
/// What an open minibuffer's submitted line means — routes the `Enter` handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinibufferKind {
    Filter,
    GotoLine,
    /// File-name input for Save As.
    SaveAs,
}

#[derive(Debug, Clone)]
pub struct Minibuffer {
    pub prompt: String,
    pub text: String,
    pub cursor: usize, // byte offset into `text`
    pub kind: MinibufferKind,
}

impl Minibuffer {
    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let prev = self.text[..self.cursor]
            .chars()
            .next_back()
            .map(char::len_utf8)
            .unwrap_or(0);
        self.cursor -= prev;
        self.text.replace_range(self.cursor..self.cursor + prev, "");
    }

    pub fn left(&mut self) {
        if self.cursor > 0 {
            let p = self.text[..self.cursor]
                .chars()
                .next_back()
                .unwrap()
                .len_utf8();
            self.cursor -= p;
        }
    }

    pub fn right(&mut self) {
        if self.cursor < self.text.len() {
            let n = self.text[self.cursor..].chars().next().unwrap().len_utf8();
            self.cursor += n;
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minibuffer_edits_text() {
        let mut m = Minibuffer { prompt: "> ".into(), text: String::new(), cursor: 0, kind: MinibufferKind::Filter };
        for c in "abc".chars() { m.insert(c); }
        assert_eq!((m.text.as_str(), m.cursor), ("abc", 3));
        m.left(); m.backspace();
        assert_eq!((m.text.as_str(), m.cursor), ("ac", 1));
    }
}
