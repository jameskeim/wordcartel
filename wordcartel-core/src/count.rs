//! Pure word/char counts (UAX-#29 word segments; alphanumeric-first rule
//! matches textobj::is_word for consistency).
use unicode_segmentation::UnicodeSegmentation;

/// Number of word segments whose first char is alphanumeric.
pub fn word_count(text: &str) -> usize {
    text.split_word_bounds()
        .filter(|seg| seg.chars().next().is_some_and(char::is_alphanumeric))
        .count()
}

/// Number of Unicode scalar values.
pub fn char_count(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_words_and_chars() {
        assert_eq!(word_count("the quick brown fox"), 4);
        assert_eq!(word_count("don't stop — now"), 3); // contraction = 1 word; em-dash not a word
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("   "), 0);
        assert_eq!(char_count("café"), 4); // 'é' is one char
        assert_eq!(char_count(""), 0);
    }
}
