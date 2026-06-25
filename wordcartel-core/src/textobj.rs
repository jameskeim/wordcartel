//! Pure word/sentence boundary queries (UAX-#29). Offsets are byte indices
//! into `text`; `pos` is clamped into `0..=text.len()`. The shell passes the
//! caret's containing leaf-block slice as `text` so work is paragraph-bounded.

use unicode_segmentation::UnicodeSegmentation;

/// A "word" segment is one whose first char is alphanumeric (punctuation and
/// whitespace runs are non-words).
fn is_word(seg: &str) -> bool {
    seg.chars().next().is_some_and(char::is_alphanumeric)
}

/// (from, to) byte range of the word at `pos`. If `pos` sits in a non-word
/// (whitespace/punctuation) run, returns the zero-width point `(pos, pos)`.
pub fn word_bounds(text: &str, pos: usize) -> (usize, usize) {
    let pos = pos.min(text.len());
    for (start, seg) in text.split_word_bound_indices() {
        let end = start + seg.len();
        if pos >= start && pos < end {
            return if is_word(seg) { (start, end) } else { (pos, pos) };
        }
    }
    (pos, pos)
}

/// Start of the next word strictly after `pos`, or `None` if none remain.
pub fn next_word_start(text: &str, pos: usize) -> Option<usize> {
    let pos = pos.min(text.len());
    text.split_word_bound_indices()
        .find(|(start, seg)| *start > pos && is_word(seg))
        .map(|(start, _)| start)
}

/// Start of the word before `pos`, or `None` if at/before the first word.
pub fn prev_word_start(text: &str, pos: usize) -> Option<usize> {
    let pos = pos.min(text.len());
    text.split_word_bound_indices()
        .filter(|(start, seg)| *start < pos && is_word(seg))
        .next_back()
        .map(|(start, _)| start)
}

/// (from, to) byte range of the sentence containing `pos`, scoped to `text`.
pub fn sentence_bounds(text: &str, pos: usize) -> (usize, usize) {
    let pos = pos.min(text.len());
    let mut last_sentence = (pos, pos);
    for (start, seg) in text.split_sentence_bound_indices() {
        let end = start + seg.len();
        last_sentence = (start, end);
        if pos >= start && pos < end {
            return (start, end);
        }
    }
    last_sentence
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_bounds_inside_word() {
        // "the quick" — pos 5 is inside "quick" (bytes 4..9)
        assert_eq!(word_bounds("the quick", 5), (4, 9));
    }
    #[test]
    fn word_bounds_contraction_is_one_word() {
        // UAX-#29 keeps "don't" together
        assert_eq!(word_bounds("don't stop", 2), (0, 5));
    }
    #[test]
    fn word_bounds_in_whitespace_is_point() {
        // pos 3 is the space between "the" and "x"
        assert_eq!(word_bounds("the x", 3), (3, 3));
    }
    #[test]
    fn word_bounds_multibyte() {
        // "café x" — 'é' is 2 bytes; "café" spans 0..5
        assert_eq!(word_bounds("café x", 2), (0, 5));
    }
    #[test]
    fn next_and_prev_word_start() {
        let t = "alpha beta gamma";
        assert_eq!(next_word_start(t, 0), Some(6));   // start of "beta"
        assert_eq!(next_word_start(t, 6), Some(11));  // start of "gamma"
        assert_eq!(next_word_start(t, 11), None);     // no further word
        assert_eq!(prev_word_start(t, 16), Some(11)); // back to "gamma"
        assert_eq!(prev_word_start(t, 6), Some(0));   // back to "alpha"
        assert_eq!(prev_word_start(t, 0), None);
    }
    #[test]
    fn sentence_bounds_basic() {
        // Two sentences; pos inside the second
        let t = "One two. Three four.";
        assert_eq!(sentence_bounds(t, 12), (9, 20)); // "Three four."
        assert_eq!(sentence_bounds(t, 2), (0, 9));   // "One two. "
    }
    #[test]
    fn empty_window_is_safe() {
        assert_eq!(word_bounds("", 0), (0, 0));
        assert_eq!(next_word_start("", 0), None);
        assert_eq!(prev_word_start("", 0), None);
        assert_eq!(sentence_bounds("", 0), (0, 0));
    }
}
