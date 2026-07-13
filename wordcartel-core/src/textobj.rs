//! Pure word/sentence boundary queries (UAX-#29). Offsets are byte indices
//! into `text`; `pos` is clamped into `0..=text.len()`. The shell passes the
//! caret's containing leaf-block slice as `text` so work is paragraph-bounded.

use unicode_segmentation::{UnicodeSegmentation, USentenceBoundIndices};

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
        .rfind(|(start, seg)| *start < pos && is_word(seg))
        .map(|(start, _)| start)
}

// ── S5: sentence detection — a four-rule post-pass over UAX-29 ──────────────────
// UAX-#29 gets most boundaries right; four rules fix its known failure modes.
// R1 abbreviation/initial merge · R2 hard-wrap merge · R3 lowercase-continuation
// merge · R4 shift past closing markup. A Markdown semantic hard break vetoes ALL
// merging (protects authored verse/address lines). All allocation-free.

/// Closing markup/punctuation that may trail a sentence terminator (R4).
const CLOSERS: &[char] = &['*', '_', '`', ')', ']', '"', '\'', '”', '’', '»'];

/// Sentence terminators the post-pass recognizes (ASCII + CJK full stops).
const TERMINATORS: &[char] = &['.', '?', '!', '…', '。', '！', '？'];

/// Class-1 abbreviations — ALWAYS merge across the following break (R1). Titles,
/// name/place prefixes, citation forms — prefixes to what follows, essentially
/// never sentence-final. Matched case-insensitively (ASCII) on the previous span's
/// last token minus its final `.`. Class-2 suffix abbreviations (`co inc ltd etc`)
/// are DELIBERATELY ABSENT (a following capital is a real boundary; a lowercase
/// continuation already merges via UAX SB8 / R3); `no` is dropped.
const ABBREV_ALWAYS_MERGE: &[&str] = &[
    // titles
    "mr", "mrs", "ms", "dr", "prof", "rev", "gen", "sr", "jr",
    // name / place prefixes
    "st", "mt", "ft",
    // citation forms
    "fig", "vol", "ch", "pp", "eq", "vs", "cf", "al",
];

/// Absolute byte end of `text[..raw_end]` minus its trailing whitespace run (the
/// content-only contract, §3).
fn content_end(text: &str, raw_end: usize) -> usize {
    text[..raw_end].trim_end_matches(char::is_whitespace).len()
}

/// Does `e` (a span's effective content) end in a terminator, ignoring trailing
/// closing markup? (`"bold.**"` counts as terminated.)
fn ends_terminated(e: &str) -> bool {
    e.trim_end_matches(|c| CLOSERS.contains(&c))
        .chars().next_back().is_some_and(|c| TERMINATORS.contains(&c))
}

/// A Markdown semantic hard break inside the gap `[cend_a, n_start)` — `  \n` (two
/// trailing spaces) or `\\\n` (backslash) before the first newline. Vetoes ALL
/// merging so an authored line break is never swallowed (§4.5).
fn semantic_hard_break(text: &str, cend_a: usize, n_start: usize) -> bool {
    let gap = &text[cend_a..n_start];
    let Some(rel_nl) = gap.find('\n') else { return false };
    let nl = cend_a + rel_nl;
    let head = text[..nl].strip_suffix('\r').unwrap_or(&text[..nl]);
    head.ends_with("  ") || head.ends_with('\\')
}

/// R1 — previous content's last token is a known abbreviation or a single capital
/// initial (`"Dr."`, `"J."`).
fn r1_merge(e: &str) -> bool {
    let tok = e.rsplit(char::is_whitespace).next().unwrap_or("");
    let Some(t0) = tok.strip_suffix('.') else { return false };
    let mut cs = t0.chars();
    let single_capital = matches!((cs.next(), cs.next()), (Some(c), None) if c.is_uppercase());
    single_capital || ABBREV_ALWAYS_MERGE.iter().any(|a| t0.eq_ignore_ascii_case(a))
}

/// R2 — the break was a line separator and the previous content is not terminated
/// (the hard-wrap fix). The semantic-hard-break veto is applied by `merges`.
fn r2_merge(text: &str, e_a: &str, cend_a: usize, n_start: usize) -> bool {
    text[cend_a..n_start].contains('\n') && !ends_terminated(e_a)
}

/// R3 — the next unit begins with a lowercase letter.
fn r3_merge(n: &str) -> bool {
    n.chars().next().is_some_and(char::is_lowercase)
}

/// R4 — if the next unit opens with a run of `CLOSERS` (followed by whitespace or
/// end) and the previous content is terminated, return the run's byte length so the
/// caller can shift the boundary past it. Else `None`.
fn r4_run(e_a: &str, n: &str) -> Option<usize> {
    let run: usize = n.chars().take_while(|c| CLOSERS.contains(c)).map(char::len_utf8).sum();
    if run == 0 { return None; }
    let after = &n[run..];
    let ok_after = after.is_empty() || after.chars().next().is_some_and(char::is_whitespace);
    (ok_after && ends_terminated(e_a)).then_some(run)
}

/// Should the next unit merge into the current span? A semantic hard break vetoes
/// all merging; otherwise any of R1/R2/R3 suffices.
fn merges(text: &str, e_a: &str, cend_a: usize, n_start: usize, n_txt: &str) -> bool {
    !semantic_hard_break(text, cend_a, n_start)
        && (r1_merge(e_a) || r2_merge(text, e_a, cend_a, n_start) || r3_merge(n_txt))
}

/// Lazily yields the content-only byte spans of `text`'s sentences (the S5 post-pass
/// over UAX-29). Allocation-free — a single UAX iterator plus one-slot pushback.
struct SentenceSpans<'a> {
    text: &'a str,
    segs: USentenceBoundIndices<'a>,
    /// One-slot pushback: an un-consumed next unit, or an R4 remainder that opens the
    /// next span. `(start, raw_end)`.
    pending: Option<(usize, usize)>,
}

impl<'a> SentenceSpans<'a> {
    fn new(text: &'a str) -> Self {
        Self { text, segs: text.split_sentence_bound_indices(), pending: None }
    }
    /// The next content-bearing unit `(start, raw_end)`; whitespace-only UAX segments
    /// are skipped (they are gap material, §4.7).
    fn pull(&mut self) -> Option<(usize, usize)> {
        if let Some(u) = self.pending.take() { return Some(u); }
        for (s, seg) in self.segs.by_ref() {
            if !seg.trim().is_empty() { return Some((s, s + seg.len())); }
        }
        None
    }
}

impl Iterator for SentenceSpans<'_> {
    type Item = (usize, usize);
    fn next(&mut self) -> Option<(usize, usize)> {
        let (a_start, a_raw0) = self.pull()?;
        let mut a_cend = content_end(self.text, a_raw0);
        while let Some((mut n_start, n_raw)) = self.pull() {
            let e_a = &self.text[a_start..a_cend];
            // R4 first: a boundary shift may consume the leading closer run of N.
            if let Some(run) = r4_run(e_a, &self.text[n_start..n_raw]) {
                a_cend = n_start + run;
                let rest = &self.text[n_start + run..n_raw];
                let rem = n_start + run + (rest.len() - rest.trim_start().len());
                if rem >= n_raw { continue; }            // N was only closers → consumed
                n_start = rem;                            // remainder is the pending N
                let e_a2 = &self.text[a_start..a_cend];
                if merges(self.text, e_a2, a_cend, n_start, &self.text[n_start..n_raw]) {
                    a_cend = content_end(self.text, n_raw);
                    continue;
                }
                self.pending = Some((n_start, n_raw));    // remainder opens the next span
                break;
            }
            if merges(self.text, e_a, a_cend, n_start, &self.text[n_start..n_raw]) {
                a_cend = content_end(self.text, n_raw);
                continue;
            }
            self.pending = Some((n_start, n_raw));         // N opens the next span
            break;
        }
        Some((a_start, a_cend))
    }
}

/// All sentence content spans of `text`, in order — the S5 post-pass over UAX-29.
///
/// Each span is `(from, to)` byte offsets with **no trailing whitespace** (§3).
/// Allocation-free and `O(bytes in text)`.
///
/// # Examples
/// ```
/// use wordcartel_core::textobj::sentence_spans;
/// let t = "Dr. Smith arrived. He left.";
/// let spans: Vec<_> = sentence_spans(t).map(|(f, e)| &t[f..e]).collect();
/// assert_eq!(spans, vec!["Dr. Smith arrived.", "He left."]);
/// ```
pub fn sentence_spans(text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    SentenceSpans::new(text)
}

/// (from, to) byte range of the sentence containing `pos`, scoped to `text`.
///
/// Total and content-only (no trailing whitespace). Attach rule: a caret in the gap
/// between sentences → the PRECEDING sentence; before the first sentence's content
/// (a window opening with whitespace) → the FOLLOWING sentence; a window with no
/// sentence content → `(0, 0)`.
pub fn sentence_bounds(text: &str, pos: usize) -> (usize, usize) {
    let pos = pos.min(text.len());
    let mut prev: Option<(usize, usize)> = None;
    for (from, to) in sentence_spans(text) {
        if pos < from {
            return prev.unwrap_or((from, to)); // gap → preceding; block start → following
        }
        if pos < to {
            return (from, to);
        }
        prev = Some((from, to));
    }
    prev.unwrap_or((0, 0)) // past last → last; no content → (0,0)
}

/// Start of the sentence STRICTLY BEFORE `pos` (the greatest sentence start `< pos`)
/// — Emacs `M-a`'s kernel. `None` at or before the first sentence start.
///
/// # Examples
/// ```
/// use wordcartel_core::textobj::prev_sentence_start;
/// assert_eq!(prev_sentence_start("One two. Three four.", 12), Some(9));
/// ```
pub fn prev_sentence_start(text: &str, pos: usize) -> Option<usize> {
    let pos = pos.min(text.len());
    let mut last = None;
    for (from, _) in sentence_spans(text) {
        if from >= pos { break; }
        last = Some(from);
    }
    last
}

/// Content end of the first sentence whose end is STRICTLY AFTER `pos` — Emacs
/// `M-e`'s kernel. `None` past the last sentence's content end.
///
/// # Examples
/// ```
/// use wordcartel_core::textobj::next_sentence_end;
/// assert_eq!(next_sentence_end("One two. Three four.", 0), Some(8));
/// ```
pub fn next_sentence_end(text: &str, pos: usize) -> Option<usize> {
    let pos = pos.min(text.len());
    sentence_spans(text).map(|(_, to)| to).find(|&to| to > pos)
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
    fn empty_window_is_safe() {
        assert_eq!(word_bounds("", 0), (0, 0));
        assert_eq!(next_word_start("", 0), None);
        assert_eq!(prev_word_start("", 0), None);
        assert_eq!(sentence_bounds("", 0), (0, 0));
    }

    // ── S5: content-only sentence_bounds (deliberate contract change) ──────────────
    #[test]
    fn sentence_bounds_basic() {
        // S5: content-only — the trailing space after the first sentence is DROPPED.
        // Was (0,9) pre-S5; now (0,8). The second sentence has no trailing space → unchanged.
        let t = "One two. Three four.";
        assert_eq!(sentence_bounds(t, 12), (9, 20)); // "Three four."
        assert_eq!(sentence_bounds(t, 2),  (0, 8));  // "One two." — content-only
    }
    #[test]
    fn sentence_bounds_attach_rule() {
        let t = "One two. Three four.";
        assert_eq!(sentence_bounds(t, 8),  (0, 8));   // gap caret → PRECEDING sentence
        assert_eq!(sentence_bounds(t, 20), (9, 20));  // pos == len → last sentence
        assert_eq!(&t[0..8], "One two.");
        // block-start: window opens with a whitespace-only hard-break line → FOLLOWING sentence
        let b = "  \nHello there. Bye.";
        let (f, _) = sentence_bounds(b, 0);
        assert!(f > 0, "leading whitespace-only line attaches to the following sentence");
    }
    #[test]
    fn sentence_spans_empty_and_whitespace() {
        assert_eq!(sentence_spans("").count(), 0);
        assert_eq!(sentence_spans("\n").count(), 0);
        assert_eq!(sentence_bounds("", 0),   (0, 0));
        assert_eq!(sentence_bounds("\n", 0), (0, 0));
    }
    #[test]
    fn r1_abbreviations_merge() {
        let spans = |t| sentence_spans(t).count();
        assert_eq!(spans("Dr. Smith arrived. He was late."), 2);   // title
        assert_eq!(spans("I saw Mt. Fuji. It was tall."), 2);      // name prefix
        assert_eq!(spans("See cf. Smith 2001. He agreed."), 2);    // citation form
        assert_eq!(spans("Smith et al. Wrote it."), 1);            // al. + capital continuation
        assert_eq!(spans("Kramer vs. Wade was long. He read it."), 2);
        assert_eq!(spans("DR. SMITH arrived. He left."), 2);       // case-insensitive
        let t = "J. R. R. Tolkien wrote it. He was English.";
        assert_eq!(spans(t), 2);                                   // single-capital initials
    }
    #[test]
    fn r1_non_abbreviations_and_dropped_no_break() {
        let spans = |t| sentence_spans(t).count();
        assert_eq!(spans("Acme Co. Then he quit."), 2);            // class-2 co + capital → break
        assert_eq!(spans("The answer was no. Then we left."), 2);  // dropped 'no' → break
        assert_eq!(spans("Q.E.D. Next problem."), 2);              // multi-char, not listed → break
    }
    #[test]
    fn r2_hard_wrap_merges() {
        let t = "The committee met on Tuesday and the\nchair insisted on a vote. Then we left.";
        let spans: Vec<_> = sentence_spans(t).map(|(f, e)| &t[f..e]).collect();
        assert_eq!(spans.len(), 2);
        assert!(spans[0].contains('\n'), "R2 merges the hard-wrapped first sentence");
    }
    #[test]
    fn hard_break_is_a_global_merge_veto() {
        // §4.5: a semantic hard break vetoes R1, R2, AND R3 — nothing merges across it.
        // Two-space hard break — NOT merged (would-be R2 continuation, capital).
        assert_eq!(sentence_spans("Roses are red,  \nViolets are blue.").count(), 2);
        // Backslash hard break — NOT merged even with a LOWERCASE continuation (vetoes R3;
        // without the global veto R3 would re-merge "verse two").
        assert_eq!(sentence_spans("verse one is red\\\nverse two is blue").count(), 2);
        // Locking fixtures: the veto also gates R1 (abbreviation before an authored hard break).
        assert_eq!(sentence_spans("Dr.  \nSmith went home.").count(), 2);   // two-space + Capital
        assert_eq!(sentence_spans("See fig.\\\nTwo shows it.").count(), 2); // backslash
        // Control: a single trailing space is a soft wrap → merged.
        assert_eq!(sentence_spans("The soft wrap ends here \nand continues.").count(), 1);
    }
    #[test]
    fn hard_break_veto_handles_crlf() {
        // I-1: the veto must strip a trailing \r before checking for the two-space/backslash
        // marker, else a CRLF file's marker byte (immediately before \r\n) is silently missed.
        assert_eq!(sentence_spans("Roses are red,  \r\nViolets are blue.").count(), 2); // two-space
        assert_eq!(sentence_spans("A line\\\r\nbroken hard.").count(), 2);              // backslash
        // Control: a CRLF WITHOUT the hard-break marker is a plain hard wrap → still merges.
        assert_eq!(sentence_spans("one two\r\nthree four").count(), 1);
    }
    #[test]
    fn r3_lowercase_after_quote_merges() {
        assert_eq!(sentence_spans("“Why?” he asked.").count(), 1);
        assert_eq!(sentence_spans("He shouted “Stop!” and ran.").count(), 1);
        assert_eq!(sentence_spans("He left. Then she left.").count(), 2); // capital control
    }
    #[test]
    fn r4_shifts_past_closing_markup() {
        let t = "This is **bold.** And this is next.";
        let spans: Vec<_> = sentence_spans(t).map(|(f, e)| &t[f..e]).collect();
        assert_eq!(spans, vec!["This is **bold.**", "And this is next."]);
        assert_eq!(sentence_spans("It was _quiet._ Then he left.").count(), 2);
        // Closers at end of text: one span, closers included.
        let e = "This is **bold.**";
        assert_eq!(sentence_spans(e).map(|(f, t)| &e[f..t]).collect::<Vec<_>>(), vec!["This is **bold.**"]);
    }
    #[test]
    fn uax_wins_are_preserved() {
        // The post-pass must not regress cases UAX already gets right.
        for t in ["We met at 10 a.m. and left.", "The U.S.A. is large.",
                  "Pi is 3.14 exactly.", "It cost $4.50 total.", "See p. 5 and fig. 2 for details."] {
            assert_eq!(sentence_spans(t).count(), 1, "UAX single-sentence case regressed: {t:?}");
        }
    }
    #[test]
    fn multibyte_offsets_are_safe() {
        assert_eq!(sentence_spans("été fini. Then done.").count(), 2);
        assert_eq!(sentence_spans("中文。Then done.").count(), 2);       // ideographic stop
        assert_eq!(sentence_spans("Nice 🙂. Then done.").count(), 2);
    }
    #[test]
    fn sentence_motion_kernels() {
        let t = "One two. Three four."; // spans (0,8), (9,20)
        // prev_sentence_start = greatest start strictly < pos (Emacs M-a kernel).
        assert_eq!(prev_sentence_start(t, 12), Some(9)); // inside 2nd → its start
        assert_eq!(prev_sentence_start(t, 9),  Some(0)); // AT 2nd start → previous
        assert_eq!(prev_sentence_start(t, 3),  Some(0)); // inside 1st → its start
        assert_eq!(prev_sentence_start(t, 0),  None);    // at doc start → none
        // next_sentence_end = first content end strictly > pos (Emacs M-e kernel).
        assert_eq!(next_sentence_end(t, 0),  Some(8));   // → 1st content end
        assert_eq!(next_sentence_end(t, 8),  Some(20));  // AT 1st end → next end
        assert_eq!(next_sentence_end(t, 9),  Some(20));  // in 2nd → its end
        assert_eq!(next_sentence_end(t, 20), None);      // at last end → none
    }
    #[test]
    fn sentence_motion_kernels_empty() {
        assert_eq!(prev_sentence_start("", 0), None);
        assert_eq!(next_sentence_end("", 0), None);
    }
}
