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

/// Words, sentences, and chars over one text window — the SP-7 shared stats helper. Sentences via
/// `crate::textobj::sentence_spans` (content-only); words/chars via the existing counters. The
/// status segment, the S6 gutter, and `count_region` all route through this ONE helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionStats {
    /// Word count (via [`word_count`]).
    pub words: usize,
    /// Sentence count (via `crate::textobj::sentence_spans`).
    pub sentences: usize,
    /// Char count (via [`char_count`]).
    pub chars: usize,
}

/// Compute [`RegionStats`] over `text`: word count, sentence count, and char count.
///
/// # Examples
/// ```
/// use wordcartel_core::count::region_stats;
/// let s = region_stats("One two. Three four five.");
/// assert_eq!(s.words, 5);
/// assert_eq!(s.sentences, 2);
/// ```
pub fn region_stats(text: &str) -> RegionStats {
    RegionStats {
        words: word_count(text),
        sentences: crate::textobj::sentence_spans(text).count(),
        chars: char_count(text),
    }
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

    #[test]
    fn region_stats_words_sentences_chars() {
        let s = region_stats("One two. Three four five.");
        assert_eq!(s.words, 5);
        assert_eq!(s.sentences, 2);
        assert_eq!(s.chars, "One two. Three four five.".chars().count());
        let z = region_stats("");
        assert_eq!((z.words, z.sentences, z.chars), (0, 0, 0));
    }

    #[test]
    fn region_stats_words_matches_word_count_for_a_single_sentence() {
        // The gutter uses count::word_count per sentence; region_stats.words must agree (one source).
        let s = "The committee met on Tuesday.";
        assert_eq!(region_stats(s).words, word_count(s));
    }
}
