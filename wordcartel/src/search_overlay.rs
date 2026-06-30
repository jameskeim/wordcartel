//! Search/replace overlay state (spec §3.2). Holds a version-keyed match cache:
//! `all_matches` is recomputed only when (needle, mode, case, buffer version)
//! changes, so highlight/count/next/prev are cheap per frame.
use ropey::Rope;
use wordcartel_core::search::{self, CaseMode, Match, Matcher, QueryMode};
use crate::editor::BufferId;

#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum Phase { Find, Replace, Stepping }
#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum Field { Needle, Template }
#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum Direction { Forward, Backward }

pub struct SearchState {
    pub phase: Phase,
    pub field: Field,
    pub needle: String,
    pub template: String,
    pub cursor: usize,            // byte caret in the focused field
    pub mode: QueryMode,
    pub case: CaseMode,
    pub direction: Direction,
    pub origin: usize,
    pub wrapped: bool,
    pub error: Option<String>,
    pub buffer_id: BufferId,
    // cache:
    matcher: Option<Matcher>,
    matches: Vec<Match>,
    capped: bool,
    cur_idx: Option<usize>,
    cache_sig: Option<(String, QueryMode, CaseMode, u64)>,
}

impl std::fmt::Debug for SearchState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchState")
            .field("phase", &self.phase)
            .field("field", &self.field)
            .field("needle", &self.needle)
            .field("template", &self.template)
            .field("cursor", &self.cursor)
            .field("mode", &self.mode)
            .field("case", &self.case)
            .field("direction", &self.direction)
            .field("origin", &self.origin)
            .field("wrapped", &self.wrapped)
            .field("error", &self.error)
            .field("buffer_id", &self.buffer_id)
            .field("matches", &self.matches)
            .field("capped", &self.capped)
            .field("cur_idx", &self.cur_idx)
            .field("cache_sig", &self.cache_sig)
            .finish_non_exhaustive()
    }
}

impl SearchState {
    pub fn open(phase: Phase, origin: usize, buffer_id: BufferId) -> SearchState {
        SearchState {
            phase, field: Field::Needle, needle: String::new(), template: String::new(),
            cursor: 0, mode: QueryMode::Literal, case: CaseMode::Smart,
            direction: Direction::Forward, origin, wrapped: false, error: None, buffer_id,
            matcher: None, matches: Vec::new(), capped: false, cur_idx: None, cache_sig: None,
        }
    }

    fn field_mut(&mut self) -> &mut String {
        match self.field { Field::Needle => &mut self.needle, Field::Template => &mut self.template }
    }
    pub fn focused_field(&self) -> &str {
        match self.field { Field::Needle => &self.needle, Field::Template => &self.template }
    }

    // Field editing — codepoint-safe, mirrors Minibuffer.
    pub fn insert(&mut self, c: char) { let i = self.cursor; self.field_mut().insert(i, c); self.cursor += c.len_utf8(); self.cache_sig = None; }
    pub fn backspace(&mut self) {
        if self.cursor == 0 { return; }
        let f = self.focused_field();
        let prev = f[..self.cursor].chars().next_back().map(char::len_utf8).unwrap_or(0);
        self.cursor -= prev; let i = self.cursor; self.field_mut().replace_range(i..i + prev, "");
        self.cache_sig = None;
    }
    pub fn left(&mut self) { if self.cursor > 0 { let p = self.focused_field()[..self.cursor].chars().next_back().unwrap().len_utf8(); self.cursor -= p; } }
    pub fn right(&mut self) { let f = self.focused_field(); if self.cursor < f.len() { self.cursor += f[self.cursor..].chars().next().unwrap().len_utf8(); } }

    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode { QueryMode::Literal => QueryMode::Regex, QueryMode::Regex => QueryMode::Literal };
        self.cache_sig = None;
    }
    pub fn cycle_case(&mut self) {
        self.case = match self.case { CaseMode::Smart => CaseMode::Sensitive, CaseMode::Sensitive => CaseMode::Insensitive, CaseMode::Insensitive => CaseMode::Smart };
        self.cache_sig = None;
    }

    /// Rebuild the cache iff (needle, mode, case, version) changed since last call.
    pub fn recompute(&mut self, rope: &Rope, version: u64) {
        let sig = (self.needle.clone(), self.mode, self.case, version);
        if self.cache_sig.as_ref() == Some(&sig) { return; }
        self.cache_sig = Some(sig);
        self.error = None; self.matches.clear(); self.capped = false; self.matcher = None; self.cur_idx = None;
        if self.needle.is_empty() { return; }
        match search::compile(&self.needle, self.mode, self.case) {
            Ok(m) => {
                let (matches, capped) = search::all_matches(rope, &m, crate::limits::MAX_SEARCH_MATCHES);
                self.matches = matches; self.capped = capped; self.matcher = Some(m);
                self.cur_idx = self.first_at_or_after(self.origin);
            }
            Err(e) => { self.error = Some(e.0); }
        }
    }

    fn first_at_or_after(&self, off: usize) -> Option<usize> {
        if self.matches.is_empty() { return None; }
        Some(self.matches.iter().position(|m| m.start >= off).unwrap_or(0)) // wrap to first
    }

    pub fn count(&self) -> usize { self.matches.len() }
    pub fn capped(&self) -> bool { self.capped }
    pub fn current(&self) -> Option<Match> { self.cur_idx.map(|i| self.matches[i]) }
    pub fn current_ordinal(&self) -> Option<usize> { self.cur_idx.map(|i| i + 1) }
    pub fn matcher(&self) -> Option<&Matcher> { self.matcher.as_ref() }
    pub fn matches(&self) -> &[Match] { &self.matches }

    pub fn cache_invalidate(&mut self) { self.cache_sig = None; }
    pub fn set_current_at_or_after(&mut self, off: usize) { self.cur_idx = self.first_at_or_after_strict(off); }
    fn first_at_or_after_strict(&self, off: usize) -> Option<usize> {
        self.matches.iter().position(|m| m.start >= off) // None when past the last match → stepping ends
    }

    pub fn next(&mut self) -> Option<Match> {
        if self.matches.is_empty() { return None; }
        self.direction = Direction::Forward;
        let i = self.cur_idx.map(|i| i + 1).unwrap_or(0);
        self.wrapped = i >= self.matches.len();
        let i = i % self.matches.len();
        self.cur_idx = Some(i); Some(self.matches[i])
    }
    pub fn prev(&mut self) -> Option<Match> {
        if self.matches.is_empty() { return None; }
        self.direction = Direction::Backward;
        let i = match self.cur_idx { Some(0) | None => { self.wrapped = true; self.matches.len() - 1 }
                                     Some(i) => { self.wrapped = false; i - 1 } };
        self.cur_idx = Some(i); Some(self.matches[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ropey::Rope;
    use crate::editor::BufferId;

    fn st(needle: &str) -> SearchState {
        let mut s = SearchState::open(Phase::Find, 0, BufferId(1));
        for c in needle.chars() { s.insert(c); }
        s
    }

    #[test]
    fn cache_recomputes_on_needle_and_version() {
        let rope = Rope::from_str("aa aa\n");
        let mut s = st("aa");
        s.recompute(&rope, 0);
        assert_eq!(s.count(), 2);
        // same key → no rescan needed but count stable
        s.recompute(&rope, 0);
        assert_eq!(s.count(), 2);
        // version bump (an edit happened) → recompute against new rope
        let rope2 = Rope::from_str("aa\n");
        s.recompute(&rope2, 1);
        assert_eq!(s.count(), 1);
    }

    #[test]
    fn current_is_first_match_at_or_after_origin() {
        let rope = Rope::from_str("aa aa aa\n");
        let mut s = SearchState::open(Phase::Find, 3, BufferId(1)); // origin past first
        for c in "aa".chars() { s.insert(c); }
        s.recompute(&rope, 0);
        assert_eq!(s.current(), Some(wordcartel_core::search::Match { start: 3, end: 5 }));
    }

    #[test]
    fn next_wraps_and_sets_flag() {
        let rope = Rope::from_str("aa aa\n");
        let mut s = st("aa"); s.recompute(&rope, 0);
        assert_eq!(s.current().unwrap().start, 0);
        s.next(); assert_eq!(s.current().unwrap().start, 3); assert!(!s.wrapped);
        s.next(); assert_eq!(s.current().unwrap().start, 0); assert!(s.wrapped); // wrapped to top
    }

    #[test]
    fn cycle_case_is_smart_sensitive_insensitive() {
        let mut s = st("x");
        assert_eq!(s.case, CaseMode::Smart);
        s.cycle_case(); assert_eq!(s.case, CaseMode::Sensitive);
        s.cycle_case(); assert_eq!(s.case, CaseMode::Insensitive);
        s.cycle_case(); assert_eq!(s.case, CaseMode::Smart);
    }

    #[test]
    fn invalid_regex_sets_error_and_zero_matches() {
        let rope = Rope::from_str("abc\n");
        let mut s = st("("); s.toggle_mode(); // → Regex
        s.recompute(&rope, 0);
        assert!(s.error.is_some());
        assert_eq!(s.count(), 0);
    }

    #[test]
    fn capped_is_false_when_all_matches_returned() {
        let rope = Rope::from_str("aa aa aa\n");
        let mut s = st("aa");
        s.recompute(&rope, 0);
        assert_eq!(s.count(), 3);
        assert!(!s.capped(), "small doc with 3 matches is not capped");
    }
}
