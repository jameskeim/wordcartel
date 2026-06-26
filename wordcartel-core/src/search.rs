//! In-document search engine (spec §3.1). Runs `regex-cursor` over the rope —
//! search iteration is allocation-free; only `expand_replacement` (Task 2) may
//! materialize the single matched region. Oracle-tested vs the `regex` crate.
use ropey::Rope;
// SEARCH engine: cursor-streamed over the rope (Codex Critical — these are the
// regex-CURSOR types, NOT regex-automata's byte-slice Input/Regex).
use regex_cursor::{Input as CursorInput, RopeyCursor};
use regex_cursor::engines::meta::Regex as CursorRegex;
// CAPTURE engine: regex-automata's meta::Regex, used by expand_replacement
// (Task 2) over a MATERIALIZED &str region (regex-cursor has no captures path).
use regex_automata::meta::Regex as AutomataRegex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryMode { Literal, Regex }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseMode { Smart, Sensitive, Insensitive }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match { pub start: usize, pub end: usize }

/// Holds BOTH engines built from the same resolved pattern: the cursor engine
/// for rope search, the automata engine for capture interpolation (Task 2).
pub struct Matcher {
    search: CursorRegex,
    /// Reserved for Task 2's `expand_replacement` over a materialized &str.
    captures: AutomataRegex,
}

/// Build a matcher. Literal mode escapes the needle (`regex_syntax::escape`).
/// Smart-case resolves to Insensitive unless `needle` has an uppercase letter.
pub fn compile(needle: &str, mode: QueryMode, case: CaseMode) -> Result<Matcher, CompileError> {
    let pattern = match mode {
        QueryMode::Literal => regex_syntax::escape(needle),
        QueryMode::Regex => needle.to_string(),
    };
    let insensitive = match case {
        CaseMode::Insensitive => true,
        CaseMode::Sensitive => false,
        CaseMode::Smart => !needle.chars().any(|c| c.is_uppercase()),
    };
    // (?i) prefix toggles case-insensitivity in both meta engines.
    let full = if insensitive { format!("(?i){pattern}") } else { pattern };
    let search = CursorRegex::new(&full).map_err(|e| CompileError(e.to_string()))?;
    let captures = AutomataRegex::new(&full).map_err(|e| CompileError(e.to_string()))?;
    Ok(Matcher { search, captures })
}

/// All non-overlapping matches over the whole rope, left-to-right.
pub fn all_matches(rope: &Rope, m: &Matcher) -> Vec<Match> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let end = rope.len_bytes();
    loop {
        match next_from(rope, m, at) {
            Some(mm) => {
                // Zero-width: advance past the match to the next char boundary so
                // we never re-find the same empty match (Codex: do NOT clamp back
                // to len — advance strictly forward).
                let advance = if mm.end > mm.start { mm.end } else { next_boundary(rope, mm.end) };
                out.push(mm);
                // advance>end is the real EOF terminator (next_boundary returns len+1); advance<=at is an infallibility guard.
                if advance > end || advance <= at { break; }
                at = advance;
            }
            None => break,
        }
    }
    out
}

/// First match with `start >= from`.
pub fn find_next(rope: &Rope, m: &Matcher, from: usize) -> Option<Match> {
    next_from(rope, m, from.min(rope.len_bytes()))
}

/// Lowest-level: first match at/after byte `from`, searched via a RopeyCursor.
/// This mirrors the canonical regex-cursor docs.rs example (see Prior Art).
fn next_from(rope: &Rope, m: &Matcher, from: usize) -> Option<Match> {
    let cursor = RopeyCursor::at(rope.slice(..), from);
    let input = CursorInput::new(cursor).range(from..rope.len_bytes());
    m.search.find(input).map(|hit| Match { start: hit.start(), end: hit.end() })
}

/// Expand `$1`..`$9` / `${name}` against the captures of the match at `at`.
/// Literal mode returns `template` verbatim. Only the matched region is
/// materialized (bounded) — never the whole document (spec §3.1).
///
/// Note: we use custom single-digit parsing so that `$9y` is parsed as
/// group-ref `$9` followed by literal `y`, not as a greedy name `9y`.
/// regex-automata's `interpolate_string_into` uses greedy cap-letter parsing
/// which would consume `y` as part of the name — breaking the spec contract.
pub fn expand_replacement(rope: &Rope, m: &Matcher, at: &Match, template: &str, mode: QueryMode) -> String {
    if matches!(mode, QueryMode::Literal) {
        return template.to_string();
    }
    // Materialize ONLY the matched region, run the AUTOMATA engine in capture
    // mode against it (regex-cursor has no captures path — Codex Critical), and
    // interpolate. Offsets within `region` are match-relative.
    let region: String = rope.slice(rope.byte_to_char(at.start)..rope.byte_to_char(at.end)).to_string();
    let mut caps = m.captures.create_captures();
    m.captures.captures(regex_automata::Input::new(region.as_str()), &mut caps);
    interpolate_single_digit(template, &caps, &region)
}

/// Interpolate `$1`..`$9` / `${name}` as capture group references.
/// `$$` becomes `$`. Unknown/out-of-range groups expand to empty string.
/// Unlike regex-automata's built-in interpolation, unbraced `$N` is
/// exactly ONE digit, so `$9y` → `(group 9)(literal y)`.
fn interpolate_single_digit(
    template: &str,
    caps: &regex_automata::util::captures::Captures,
    region: &str,
) -> String {
    let bytes = template.as_bytes();
    let mut dst = String::with_capacity(template.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'$' {
            // Emit next run of non-$ bytes.
            let start = i;
            while i < bytes.len() && bytes[i] != b'$' { i += 1; }
            dst.push_str(&template[start..i]);
            continue;
        }
        // bytes[i] == b'$'
        match bytes.get(i + 1).copied() {
            Some(b'$') => {
                // Escaped dollar.
                dst.push('$');
                i += 2;
            }
            Some(b'{') => {
                // Braced reference: ${ref}.
                let name_start = i + 2;
                match bytes[name_start..].iter().position(|&b| b == b'}') {
                    None => {
                        // Unclosed brace — treat '$' as literal.
                        dst.push('$');
                        i += 1;
                    }
                    Some(close_rel) => {
                        let name = &template[name_start..name_start + close_rel];
                        let group_text = if let Ok(idx) = name.parse::<usize>() {
                            caps.get_group(idx).map(|s| &region[s.start..s.end]).unwrap_or("")
                        } else if let Some(pid) = caps.pattern() {
                            caps.group_info()
                                .to_index(pid, name)
                                .and_then(|idx| caps.get_group(idx))
                                .map(|s| &region[s.start..s.end])
                                .unwrap_or("")
                        } else {
                            ""
                        };
                        dst.push_str(group_text);
                        i = name_start + close_rel + 1; // past closing '}'
                    }
                }
            }
            Some(d @ b'1'..=b'9') => {
                // Single-digit numeric group reference ($1..$9 only).
                let idx = (d - b'0') as usize;
                if let Some(span) = caps.get_group(idx) {
                    dst.push_str(&region[span.start..span.end]);
                }
                // else: out-of-range → empty string
                i += 2;
            }
            _ => {
                // '$' followed by anything else — emit '$' literally.
                dst.push('$');
                i += 1;
            }
        }
    }
    dst
}

/// Next UTF-8 char boundary strictly after `pos` (zero-width progress).
fn next_boundary(rope: &Rope, pos: usize) -> usize {
    if pos >= rope.len_bytes() { return rope.len_bytes() + 1; } // forces loop exit
    // Defensive: regex-cursor yields char-aligned matches, but never panic if a
    // future caller passes a mid-codepoint offset — just step forward one byte.
    let Ok(ch) = rope.try_byte_to_char(pos) else { return pos + 1; };
    let next_char = (ch + 1).min(rope.len_chars());
    rope.char_to_byte(next_char).max(pos + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;

    // Oracle: our rope search == the `regex` crate on the materialized string.
    // (regex is a dev-dependency for the oracle only — see Step 6.)
    fn oracle_all(text: &str, pat: &str, ci: bool) -> Vec<(usize, usize)> {
        let re = regex::RegexBuilder::new(pat).case_insensitive(ci).build().unwrap();
        re.find_iter(text).map(|m| (m.start(), m.end())).collect()
    }

    #[test]
    fn literal_escapes_metacharacters() {
        // "a.c" in literal mode matches the literal "a.c", NOT "abc".
        let rope = Rope::from_str("a.c abc\n");
        let m = compile("a.c", QueryMode::Literal, CaseMode::Sensitive).unwrap();
        assert_eq!(all_matches(&rope, &m), vec![Match { start: 0, end: 3 }]);
    }

    #[test]
    fn regex_mode_is_raw() {
        let rope = Rope::from_str("a.c abc\n");
        let m = compile("a.c", QueryMode::Regex, CaseMode::Sensitive).unwrap();
        // matches "a.c" (0..3) and "abc" (4..7)
        assert_eq!(all_matches(&rope, &m), vec![Match { start: 0, end: 3 }, Match { start: 4, end: 7 }]);
    }

    #[test]
    fn smart_case_insensitive_when_lowercase() {
        let rope = Rope::from_str("The THE the\n");
        let m = compile("the", QueryMode::Literal, CaseMode::Smart).unwrap();
        assert_eq!(all_matches(&rope, &m).len(), 3); // matches all cases
    }

    #[test]
    fn smart_case_sensitive_when_uppercase() {
        let rope = Rope::from_str("The THE the\n");
        let m = compile("The", QueryMode::Literal, CaseMode::Smart).unwrap();
        assert_eq!(all_matches(&rope, &m), vec![Match { start: 0, end: 3 }]); // only "The"
    }

    #[test]
    fn find_next_resumes_from_offset() {
        let rope = Rope::from_str("aa aa aa\n");
        let m = compile("aa", QueryMode::Literal, CaseMode::Sensitive).unwrap();
        assert_eq!(find_next(&rope, &m, 1), Some(Match { start: 3, end: 5 }));
    }

    #[test]
    fn invalid_regex_is_compile_error() {
        assert!(compile("(", QueryMode::Regex, CaseMode::Sensitive).is_err());
    }

    #[test]
    fn zero_width_match_advances_to_char_boundary() {
        // "a*" matches empty between/around chars; must terminate and not split UTF-8.
        let rope = Rope::from_str("héllo\n"); // 'é' is 2 bytes
        let m = compile("x*", QueryMode::Regex, CaseMode::Sensitive).unwrap();
        let ms = all_matches(&rope, &m);
        // every match start is a char boundary and the scan terminates
        for mm in &ms { assert!(rope.try_byte_to_char(mm.start).is_ok()); }
        assert!(ms.len() <= rope.len_chars() + 1);
    }

    #[test]
    fn oracle_random_corpus() {
        let texts = ["", "abc\n", "héllo wörld\n", "aXbXc\nXX\n", "The quick Brown fox\n"];
        let pats = ["a", "X", "the", "\\w+", "b.c", "o"];
        for t in texts {
            for p in pats {
                for &ci in &[false, true] {
                    let case = if ci { CaseMode::Insensitive } else { CaseMode::Sensitive };
                    let Ok(m) = compile(p, QueryMode::Regex, case) else { continue };
                    let got: Vec<(usize, usize)> = all_matches(&Rope::from_str(t), &m)
                        .into_iter().map(|x| (x.start, x.end)).collect();
                    assert_eq!(got, oracle_all(t, p, ci), "text={t:?} pat={p:?} ci={ci}");
                }
            }
        }
    }

    #[test]
    fn literal_replacement_is_verbatim() {
        let rope = Rope::from_str("hello\n");
        let m = compile("hello", QueryMode::Literal, CaseMode::Sensitive).unwrap();
        let at = all_matches(&rope, &m)[0];
        // In literal mode, "$1" is literal text, not a capture ref.
        assert_eq!(expand_replacement(&rope, &m, &at, "bye $1", QueryMode::Literal), "bye $1");
    }

    #[test]
    fn regex_replacement_expands_captures() {
        let rope = Rope::from_str("Smith, John\n");
        let m = compile("(\\w+), (\\w+)", QueryMode::Regex, CaseMode::Sensitive).unwrap();
        let at = all_matches(&rope, &m)[0];
        assert_eq!(expand_replacement(&rope, &m, &at, "$2 $1", QueryMode::Regex), "John Smith");
    }

    #[test]
    fn regex_replacement_out_of_range_group_is_empty() {
        let rope = Rope::from_str("abc\n");
        let m = compile("abc", QueryMode::Regex, CaseMode::Sensitive).unwrap();
        let at = all_matches(&rope, &m)[0];
        assert_eq!(expand_replacement(&rope, &m, &at, "x$9y", QueryMode::Regex), "xy");
    }
}
