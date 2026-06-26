# Wordcartel Effort 5e — Search & Replace Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Incremental in-document search & replace — literal/regex queries, tri-state case, viewport-gated highlight-all with live count, find-next/prev with wrap, replace-all (single undo unit) and interactive query-replace.

**Architecture:** Match-finding is a new IO-free `wordcartel-core::search` module (`regex-cursor` over the rope; oracle-tested vs the `regex` crate). The shell adds a `SearchState` overlay (XOR with prompt/palette/menu/minibuffer) holding a **version-keyed match cache**, a `reduce()` interception branch, a search bar + a `ColMap`-projected highlight layer in `render.rs`, and a multi-op replace path reusing the existing `editor.apply` commit contract.

**Tech Stack:** Rust, `ropey =1.6.1`, `regex-cursor` + `regex-automata` (new core deps), `ratatui`, `crossterm`.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-06-25-wordcartel-05e-search-replace-design.md` — authoritative; this plan implements it.
- **Functional core:** `wordcartel-core` is IO/thread-free and `#![forbid(unsafe_code)]`. New deps may use unsafe internally; **our** core code may not. New core deps must be **MIT/Apache**.
- **Pinned rope:** `ropey = "=1.6.1"` — do not bump.
- **Literal is the default** query mode; `regex::escape` the needle in literal mode. Regex mode is opt-in (`Alt+R`).
- **Case:** tri-state `Smart` (default) → `Sensitive` → `Insensitive`; **smart-case is resolved inside `search::compile`** (insensitive unless the needle contains an uppercase letter) so the shell never re-derives it.
- **Highlights project through `ColMap.placed[].src`**, never raw row byte ranges (concealed-markdown alignment).
- **`origin` (Esc target) is remapped through every replacement commit's `ChangeSet` via `change::map_pos`** — like `marks`/`jump_ring` in `editor.rs`.
- **Replace-all / `!` = ONE composed `ChangeSet`** over original offsets + ONE covering `block_tree::Edit{first.start..last.end}` → ONE `editor.apply` = one undo unit. **Per-match `y` re-finds on the mutated rope.**
- **Zero-width matches** advance to the **next UTF-8 char boundary** (≥1 byte, never mid-codepoint).
- **Match cache** keyed by `(needle, mode, case, buffer.version)`; highlight/count/next/prev read the cache. Full-scan count; cap deferred.
- **Overlay XOR is not centralized** — every `open_*` clears `search`; `open_search` clears all siblings + `pending_keys` + `pending_mark` (spec §3.3).
- **Search keys are config-driven:** register the **commands**, bind them in `input::key_to_command_id` AND the CUA preset in `keymap.rs`. A preset that rebinds a key (WordStar `ctrl-f`) shadowing search is expected.
- **Default-off / inactive = true no-op:** when `editor.search` is `None`, render and reduce behave exactly as today (existing tests stay green).
- Commit trailers on every commit:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```
- Run core tests with `cargo test -p wordcartel-core`, shell with `cargo test -p wordcartel`. Zero warnings.

---

## File Structure

| File | Change | Responsibility |
|------|--------|----------------|
| `wordcartel-core/Cargo.toml` | Modify | add `regex-cursor`, `regex-automata` deps |
| `wordcartel-core/src/lib.rs` | Modify | `pub mod search;` |
| `wordcartel-core/src/search.rs` | **Create** | `compile`, `all_matches`, `find_next`, `expand_replacement`; `Matcher`, `Match`, `QueryMode`, `CaseMode`, `CompileError`. Oracle-tested. |
| `wordcartel/src/search_overlay.rs` | **Create** | `SearchState` (fields, phase/field enums), version-keyed match cache, field editing, `recompute`, `current`/`count`/`next`/`prev` helpers. |
| `wordcartel/src/editor.rs` | Modify | `search: Option<SearchState>` field; `open_search`; every `open_*` clears `search`. |
| `wordcartel/src/registry.rs` | Modify | register `find`/`replace`/`find_next`/`find_prev` commands. |
| `wordcartel/src/input.rs` | Modify | map `Ctrl+F`/`Ctrl+R`/`F3`/`Shift+F3` → command ids. |
| `wordcartel/src/keymap.rs` | Modify | mirror the same binds in the CUA preset table. |
| `wordcartel/src/app.rs` | Modify | `reduce()` search-overlay interception branch; clear `search` on buffer-swap/click-outside. |
| `wordcartel/src/render.rs` | Modify | search bar (status row) + `ColMap`-projected highlight layer. |
| `wordcartel/src/commands.rs` | Modify | `build_multi_replace` (multi-op ChangeSet + covering Edit). |

---

## Task 1: Core search engine + dependency build gate

**This is the risk gate — prove the deps build and the engine works before any feature code.**

**Files:**
- Modify: `wordcartel-core/Cargo.toml`
- Modify: `wordcartel-core/src/lib.rs`
- Create: `wordcartel-core/src/search.rs`

**Interfaces:**
- Consumes: `ropey::Rope` (re-exported as `crate::...`; tests use `ropey::Rope::from_str`).
- Produces:
  ```rust
  pub enum QueryMode { Literal, Regex }
  pub enum CaseMode { Smart, Sensitive, Insensitive }
  pub struct CompileError(pub String);
  pub struct Matcher { /* opaque */ }
  pub struct Match { pub start: usize, pub end: usize } // half-open byte range
  pub fn compile(needle: &str, mode: QueryMode, case: CaseMode) -> Result<Matcher, CompileError>;
  pub fn all_matches(rope: &ropey::Rope, m: &Matcher) -> Vec<Match>; // non-overlapping, L→R, whole doc
  pub fn find_next(rope: &ropey::Rope, m: &Matcher, from: usize) -> Option<Match>; // first match with start >= from
  ```

- [ ] **Step 1: Add dependencies**

In `wordcartel-core/Cargo.toml` under `[dependencies]`:
```toml
regex-cursor = "0.1"
regex-automata = "0.4"
```

- [ ] **Step 2: Build gate — prove it compiles against the pinned rope**

Run: `cargo build -p wordcartel-core`
Expected: PASS. If `regex-cursor` fails to resolve against `ropey =1.6.1`, STOP and report — this is the gate; do not proceed. (Contingency in spec §9.3/§11: fall back to chunked `regex` over windows.)

- [ ] **Step 3: Declare the module**

In `wordcartel-core/src/lib.rs`, add alongside the other `pub mod` lines:
```rust
pub mod search;
```

- [ ] **Step 4: Write the failing oracle + unit tests**

Create `wordcartel-core/src/search.rs` ending with:
```rust
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
}
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test -p wordcartel-core search::`
Expected: FAIL (compile errors — `compile`/`all_matches`/`find_next` not defined).

- [ ] **Step 6: Add the `regex` dev-dependency (oracle only)**

In `wordcartel-core/Cargo.toml` under `[dev-dependencies]`:
```toml
regex = "1"
```

- [ ] **Step 7: Implement `search.rs`**

```rust
//! In-document search engine (spec §3.1). Runs `regex-cursor` over the rope —
//! search iteration is allocation-free; only `expand_replacement` (Task 2) may
//! materialize the single matched region. Oracle-tested vs the `regex` crate.
use ropey::Rope;
use regex_automata::meta::Regex;
use regex_automata::Input;
use regex_cursor::RopeyCursor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryMode { Literal, Regex }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseMode { Smart, Sensitive, Insensitive }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompileError(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match { pub start: usize, pub end: usize }

pub struct Matcher { re: Regex }

/// Build a matcher. Literal mode escapes the needle. Smart-case resolves to
/// Insensitive unless `needle` contains an uppercase letter.
pub fn compile(needle: &str, mode: QueryMode, case: CaseMode) -> Result<Matcher, CompileError> {
    let pattern = match mode {
        QueryMode::Literal => regex_automata::util::escape(needle),
        QueryMode::Regex => needle.to_string(),
    };
    let insensitive = match case {
        CaseMode::Insensitive => true,
        CaseMode::Sensitive => false,
        CaseMode::Smart => !needle.chars().any(|c| c.is_uppercase()),
    };
    // (?i) prefix toggles case-insensitivity in regex-automata's meta engine.
    let full = if insensitive { format!("(?i){pattern}") } else { pattern };
    let re = Regex::builder()
        .build(&full)
        .map_err(|e| CompileError(e.to_string()))?;
    Ok(Matcher { re })
}

/// All non-overlapping matches over the whole rope, left-to-right.
pub fn all_matches(rope: &Rope, m: &Matcher) -> Vec<Match> {
    let mut out = Vec::new();
    let mut at = 0usize;
    let end = rope.len_bytes();
    while at <= end {
        match next_from(rope, m, at) {
            Some(mm) => {
                let advance = if mm.end > mm.start { mm.end } else { next_boundary(rope, mm.end) };
                out.push(mm);
                if advance <= at { break; } // safety
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

// Lowest-level: search the rope starting at byte `from` via a RopeyCursor.
fn next_from(rope: &Rope, m: &Matcher, from: usize) -> Option<Match> {
    let cursor = RopeyCursor::at(rope.slice(..), from);
    let input = Input::new(cursor).span(from..rope.len_bytes());
    m.re.search_with(&mut regex_automata::util::pool::...) // see note
        ;
    // NOTE: the exact regex-cursor call (`regex_cursor::Input` + `m.re.find`) is
    // settled at Task-1 build time against the resolved crate version; the
    // contract is: return the first Match at/after `from`, or None. Use
    // regex-cursor's documented `find`/`search` entry that accepts a RopeyCursor.
}

/// Next UTF-8 char boundary strictly after `pos` (for zero-width progress).
fn next_boundary(rope: &Rope, pos: usize) -> usize {
    if pos >= rope.len_bytes() { return pos + 1; }
    let ch = rope.byte_to_char(pos);
    let next_char = (ch + 1).min(rope.len_chars());
    rope.char_to_byte(next_char).max(pos + 1)
}
```

> Implementer note: `next_from`'s body is the one place that depends on the
> resolved `regex-cursor` surface. The build gate (Step 2) fixes the version;
> wire `RopeyCursor` + `regex_cursor::Input` to `Regex::find` per that version's
> docs. The **contract and all tests above are fixed**; only the 3-5 lines
> inside `next_from` adapt to the crate. If the resolved API differs, keep the
> signature and make the oracle test pass.

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test -p wordcartel-core search::`
Expected: PASS (all 8 tests). Zero warnings: `cargo build -p wordcartel-core 2>&1 | grep -i warning` empty.

- [ ] **Step 9: Commit**

```bash
git add wordcartel-core/Cargo.toml wordcartel-core/src/lib.rs wordcartel-core/src/search.rs
git commit -m "feat(core): search engine — regex-cursor over the rope (compile/all_matches/find_next), oracle-tested"
```

---

## Task 2: Capture-aware replacement expansion

**Files:**
- Modify: `wordcartel-core/src/search.rs`

**Interfaces:**
- Consumes: `Matcher`, `Match`, `QueryMode` (Task 1).
- Produces:
  ```rust
  pub fn expand_replacement(rope: &Rope, m: &Matcher, at: &Match, template: &str, mode: QueryMode) -> String;
  ```

- [ ] **Step 1: Write the failing tests**

Append to `search.rs` tests:
```rust
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
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p wordcartel-core search::`
Expected: FAIL (`expand_replacement` not defined).

- [ ] **Step 3: Implement `expand_replacement`**

```rust
/// Expand `$1`..`$9` / `${name}` against the captures of the match at `at`.
/// Literal mode returns `template` verbatim. Only the matched region is
/// materialized (bounded) — never the whole document (spec §3.1).
pub fn expand_replacement(rope: &Rope, m: &Matcher, at: &Match, template: &str, mode: QueryMode) -> String {
    if matches!(mode, QueryMode::Literal) {
        return template.to_string();
    }
    // Materialize ONLY the matched region, re-run in capture mode against it,
    // and interpolate. Offsets within `region` are match-relative.
    let region: String = rope.slice(rope.byte_to_char(at.start)..rope.byte_to_char(at.end)).to_string();
    let mut caps = m.re.create_captures();
    m.re.captures(&region, &mut caps);
    let mut dst = String::new();
    caps.interpolate_string_into(&region, template, &mut dst);
    dst
}
```

> Implementer note: `create_captures` / `captures` / `interpolate_string_into`
> are the `regex-automata` capture+interpolation entry points. If the resolved
> version names them differently, keep the signature and the three tests; if
> `regex-cursor` exposes no captures path at all and meta-captures on the
> region is unavailable, the documented fallback (spec §3.1) is literal-only
> replacement — implement that and mark the two regex tests `#[ignore]` with a
> comment citing the gate outcome, then raise it for the review.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p wordcartel-core search::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add wordcartel-core/src/search.rs
git commit -m "feat(core): expand_replacement — \$N capture refs in regex mode, literal verbatim otherwise"
```

---

## Task 3: SearchState overlay + version-keyed match cache + XOR wiring

**Files:**
- Create: `wordcartel/src/search_overlay.rs`
- Modify: `wordcartel/src/lib.rs` (add `pub mod search_overlay;`)
- Modify: `wordcartel/src/editor.rs` (field + `open_search`; every `open_*` clears `search`)

**Interfaces:**
- Consumes: `wordcartel_core::search::{compile, all_matches, find_next, Matcher, Match, QueryMode, CaseMode}`; `crate::editor::BufferId`.
- Produces:
  ```rust
  pub enum Phase { Find, Replace, Stepping }
  pub enum Field { Needle, Template }
  pub enum Direction { Forward, Backward }
  pub struct SearchState { /* spec §3.2 fields + cache */ }
  impl SearchState {
    pub fn open(phase: Phase, origin: usize, buffer_id: BufferId) -> SearchState;
    pub fn insert(&mut self, c: char);           // into focused field
    pub fn backspace(&mut self);
    pub fn left(&mut self);
    pub fn right(&mut self);
    pub fn toggle_mode(&mut self);               // Alt+R
    pub fn cycle_case(&mut self);                // Alt+C
    pub fn recompute(&mut self, rope: &Rope, version: u64); // rebuild cache if key changed
    pub fn count(&self) -> usize;
    pub fn current(&self) -> Option<Match>;
    pub fn next(&mut self) -> Option<Match>;     // advance current forward (wrap), sets wrapped
    pub fn prev(&mut self) -> Option<Match>;     // backward (wrap)
    pub fn focused_field(&self) -> &str;         // &self.needle or &self.template
  }
  ```
- Editor gains `pub search: Option<crate::search_overlay::SearchState>` and `pub fn open_search(&mut self, phase, origin)`.

- [ ] **Step 1: Write the failing tests**

Create `wordcartel/src/search_overlay.rs` ending with:
```rust
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
}
```

Add an editor-level test in `wordcartel/src/editor.rs` tests:
```rust
#[test]
fn open_search_clears_siblings_and_open_others_clear_search() {
    let mut e = Editor::new_from_text("x\n", None, (80, 24));
    e.open_minibuffer("> ");
    e.open_search(crate::search_overlay::Phase::Find, 0);
    assert!(e.search.is_some() && e.minibuffer.is_none() && e.prompt.is_none()
            && e.palette.is_none() && e.menu.is_none());
    e.open_palette();
    assert!(e.search.is_none(), "open_palette must clear search");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p wordcartel search_overlay:: ; cargo test -p wordcartel open_search_clears`
Expected: FAIL (types/methods not defined).

- [ ] **Step 3: Implement `SearchState`**

```rust
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
    cur_idx: Option<usize>,
    cache_sig: Option<(String, QueryMode, CaseMode, u64)>,
}

impl SearchState {
    pub fn open(phase: Phase, origin: usize, buffer_id: BufferId) -> SearchState {
        SearchState {
            phase, field: Field::Needle, needle: String::new(), template: String::new(),
            cursor: 0, mode: QueryMode::Literal, case: CaseMode::Smart,
            direction: Direction::Forward, origin, wrapped: false, error: None, buffer_id,
            matcher: None, matches: Vec::new(), cur_idx: None, cache_sig: None,
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
        self.error = None; self.matches.clear(); self.matcher = None; self.cur_idx = None;
        if self.needle.is_empty() { return; }
        match search::compile(&self.needle, self.mode, self.case) {
            Ok(m) => { self.matches = search::all_matches(rope, &m); self.matcher = Some(m);
                       self.cur_idx = self.first_at_or_after(self.origin); }
            Err(e) => { self.error = Some(e.0); }
        }
    }

    fn first_at_or_after(&self, off: usize) -> Option<usize> {
        if self.matches.is_empty() { return None; }
        Some(self.matches.iter().position(|m| m.start >= off).unwrap_or(0)) // wrap to first
    }

    pub fn count(&self) -> usize { self.matches.len() }
    pub fn current(&self) -> Option<Match> { self.cur_idx.map(|i| self.matches[i]) }
    pub fn current_ordinal(&self) -> Option<usize> { self.cur_idx.map(|i| i + 1) }
    pub fn matcher(&self) -> Option<&Matcher> { self.matcher.as_ref() }
    pub fn matches(&self) -> &[Match] { &self.matches }

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
```

- [ ] **Step 4: Wire the editor field + openers**

In `wordcartel/src/editor.rs`:
- Add field to `Editor`: `pub search: Option<crate::search_overlay::SearchState>,`
- Initialize in `new_from_text`: `search: None,`
- Add the opener (mirror `open_minibuffer`'s clearing exactly):
```rust
pub fn open_search(&mut self, phase: crate::search_overlay::Phase, origin: usize) {
    self.prompt = None; self.minibuffer = None; self.palette = None; self.menu = None;
    self.pending_keys.clear(); self.pending_mark = None;
    let bid = self.active().id;
    self.search = Some(crate::search_overlay::SearchState::open(phase, origin, bid));
}
```
- In `open_minibuffer`, `open_prompt`, `open_palette` add `self.search = None;` alongside the other sibling-clears.

In `wordcartel/src/lib.rs` add: `pub mod search_overlay;`

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p wordcartel search_overlay:: ; cargo test -p wordcartel open_search_clears`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/search_overlay.rs wordcartel/src/lib.rs wordcartel/src/editor.rs
git commit -m "feat(search): SearchState overlay + version-keyed match cache + XOR wiring"
```

---

## Task 4: reduce() interception — open, incremental find, navigate, toggles, Esc-to-origin + keybindings

**Files:**
- Modify: `wordcartel/src/registry.rs` (register commands)
- Modify: `wordcartel/src/input.rs` (key → id)
- Modify: `wordcartel/src/keymap.rs` (CUA preset)
- Modify: `wordcartel/src/app.rs` (`reduce()` interception)

**Interfaces:**
- Consumes: `SearchState` (Task 3); `Editor::open_search`; `nav::ensure_visible`, `derive::rebuild`; `Selection::{single, range}`.
- Produces: command ids `"find"`, `"replace"`, `"find_next"`, `"find_prev"`; a `reduce()` branch handling `editor.search.is_some()`.

- [ ] **Step 1: Register the commands**

In `registry.rs` `builtins()`, near the `filter` registration:
```rust
r.register("find", "Find…", Some(MenuCategory::Edit), |c| {
    let origin = c.editor.active().document.selection.primary().to();
    c.editor.open_search(crate::search_overlay::Phase::Find, origin);
    CommandResult::Handled
});
r.register("replace", "Replace…", Some(MenuCategory::Edit), |c| {
    let origin = c.editor.active().document.selection.primary().to();
    c.editor.open_search(crate::search_overlay::Phase::Replace, origin);
    CommandResult::Handled
});
// find_next / find_prev are no-ops unless the overlay is open (handled in reduce);
// register them so they appear in the palette and can be bound.
r.register("find_next", "Find Next", None, |_c| CommandResult::Handled);
r.register("find_prev", "Find Previous", None, |_c| CommandResult::Handled);
```

- [ ] **Step 2: Bind the keys (both tables)**

In `input.rs` `key_to_command_id`, in the Ctrl block (near `ctrl-e → filter`):
```rust
KeyCode::Char('f') if ctrl => id("find"),
KeyCode::Char('r') if ctrl => id("replace"),
KeyCode::F(3) if shift     => id("find_prev"),
KeyCode::F(3)              => id("find_next"),
```
In `keymap.rs`, add to the CUA preset table (mirroring `("ctrl-e","filter")`):
```rust
("ctrl-f", "find"),
("ctrl-r", "replace"),
("f3", "find_next"),
("shift-f3", "find_prev"),
```

- [ ] **Step 3: Write the failing tests**

In `app.rs` tests:
```rust
#[test]
fn ctrl_f_opens_search_and_typing_jumps() {
    use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("foo bar foo\n", None, (80, 24));
    let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    reduce(Msg::Input(press(KeyCode::Char('f'), KeyModifiers::CONTROL)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
    assert!(e.search.is_some(), "Ctrl+F opens search");
    for c in "bar".chars() { reduce(Msg::Input(press(KeyCode::Char(c), KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx); }
    let s = e.search.as_ref().unwrap();
    assert_eq!(s.needle, "bar");
    assert_eq!(s.current().unwrap().start, 4); // caret jumped to the match
    assert_eq!(e.active().document.selection.primary().from(), 4);
}

#[test]
fn esc_restores_origin_and_closes() {
    use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("foo bar\n", None, (80, 24));
    e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
    let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    reduce(Msg::Input(press(KeyCode::Char('f'), KeyModifiers::CONTROL)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
    for c in "bar".chars() { reduce(Msg::Input(press(KeyCode::Char(c), KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx); }
    reduce(Msg::Input(press(KeyCode::Esc, KeyModifiers::NONE)), &mut e, &reg, &cua_keymap(), &ex, &clk, &tx);
    assert!(e.search.is_none(), "Esc closes search");
    assert_eq!(e.active().document.selection.primary().to(), 0, "caret restored to origin");
}
```

- [ ] **Step 4: Run to verify they fail**

Run: `cargo test -p wordcartel ctrl_f_opens_search esc_restores_origin`
Expected: FAIL (no reduce branch yet — search stays open / caret doesn't move).

- [ ] **Step 5: Implement the reduce() branch**

In `app.rs` `reduce()`, insert a new interception block **immediately after the `editor.minibuffer.is_some()` block and before normal dispatch**:
```rust
// Search overlay: intercept before normal editing (spec §3.3 precedence).
if editor.search.is_some() {
    if let Msg::Input(Event::Key(k)) = &msg {
        if k.kind == crossterm::event::KeyEventKind::Press {
            use crossterm::event::{KeyCode, KeyModifiers};
            let alt = k.modifiers.contains(KeyModifiers::ALT);
            let shift = k.modifiers.contains(KeyModifiers::SHIFT);
            let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
            match k.code {
                KeyCode::Esc => { search_cancel(editor); return; }
                KeyCode::Char('r') if alt => { editor.search.as_mut().unwrap().toggle_mode(); }
                KeyCode::Char('c') if alt => { editor.search.as_mut().unwrap().cycle_case(); }
                KeyCode::Enter if shift => { search_step(editor, false); }
                KeyCode::F(3) if shift   => { search_step(editor, false); }
                KeyCode::Enter           => { search_step(editor, true); }
                KeyCode::F(3)            => { search_step(editor, true); }
                KeyCode::Backspace       => { editor.search.as_mut().unwrap().backspace(); }
                KeyCode::Left            => { editor.search.as_mut().unwrap().left(); }
                KeyCode::Right           => { editor.search.as_mut().unwrap().right(); }
                KeyCode::Char(c) if !ctrl => { editor.search.as_mut().unwrap().insert(c); }
                _ => {}
            }
            // Recompute against the live buffer and pin the current match.
            search_sync(editor);
        }
    }
    return;
}
```
Add these helpers in `app.rs` (module scope):
```rust
fn search_sync(editor: &mut Editor) {
    let (rope, version) = { let d = &editor.active().document; (d.buffer.snapshot(), d.version) };
    if let Some(s) = editor.search.as_mut() { s.recompute(&rope, version); }
    if let Some(m) = editor.search.as_ref().and_then(|s| s.current()) {
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::range(m.start, m.end);
        derive::rebuild(editor);
        crate::nav::ensure_visible(editor);
    }
}
fn search_step(editor: &mut Editor, forward: bool) {
    if let Some(s) = editor.search.as_mut() { if forward { s.next(); } else { s.prev(); } }
    if let Some(m) = editor.search.as_ref().and_then(|s| s.current()) {
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::range(m.start, m.end);
        derive::rebuild(editor);
        crate::nav::ensure_visible(editor);
    }
}
fn search_cancel(editor: &mut Editor) {
    let origin = editor.search.as_ref().map(|s| s.origin).unwrap_or(0);
    editor.search = None;
    editor.active_mut().document.selection = wordcartel_core::selection::Selection::single(origin);
    derive::rebuild(editor);
    crate::nav::ensure_visible(editor);
}
```
Also: in the buffer-swap path and the mouse click-outside path (where other overlays are cleared), add `editor.search = None;`.

- [ ] **Step 6: Run to verify they pass**

Run: `cargo test -p wordcartel ctrl_f_opens_search esc_restores_origin`
Expected: PASS. Then `cargo test -p wordcartel` — existing tests stay green (search inactive = no-op).

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/registry.rs wordcartel/src/input.rs wordcartel/src/keymap.rs wordcartel/src/app.rs
git commit -m "feat(search): reduce() interception — open/incremental-find/navigate/toggles/Esc-to-origin + Ctrl+F/R, F3"
```

---

## Task 5: Render — search bar + ColMap-projected highlight layer

**Files:**
- Modify: `wordcartel/src/render.rs`

**Interfaces:**
- Consumes: `editor.search` (`SearchState::{matches, current, count, current_ordinal, needle, template, mode, case, phase, error, wrapped}`); `view.line_layouts` `(Vec<VisualRow>, ColMap)`; `derive::line_start`; `ColMap.placed[]` (`src`, `row`, `text`, `style`).
- Produces: highlighted match glyphs in the editing area; a search bar on the status row.

- [ ] **Step 1: Write the failing tests**

In `render.rs` tests (use the existing `render_to_buffer` helper pattern in that file):
```rust
#[test]
fn search_highlights_matches_and_shows_count() {
    use crate::editor::Editor;
    let mut e = Editor::new_from_text("foo bar foo\n", None, (40, 6));
    e.open_search(crate::search_overlay::Phase::Find, 0);
    for c in "foo".chars() { e.search.as_mut().unwrap().insert(c); }
    let rope = e.active().document.buffer.snapshot(); let v = e.active().document.version;
    e.search.as_mut().unwrap().recompute(&rope, v);
    crate::derive::rebuild(&mut e);
    let buf = render_to_buffer(&e, 40, 6); // existing test helper
    let status = row_string(&buf, 5); // bottom row
    assert!(status.contains("Find:"), "search bar shows Find:");
    assert!(status.contains("1/2") || status.contains("2"), "shows match count, got {status:?}");
    // both "foo" occurrences carry a highlight bg somewhere on row 0
    assert!(row_has_highlight(&buf, 0), "matches highlighted");
}

#[test]
fn highlight_skips_concealed_markers_in_live_preview() {
    use crate::editor::Editor;
    let mut e = Editor::new_from_text("**bold**\n", None, (40, 6)); // LivePreview conceals **
    e.open_search(crate::search_overlay::Phase::Find, 0);
    for c in "bold".chars() { e.search.as_mut().unwrap().insert(c); }
    let rope = e.active().document.buffer.snapshot(); let v = e.active().document.version;
    e.search.as_mut().unwrap().recompute(&rope, v);
    crate::derive::rebuild(&mut e);
    let buf = render_to_buffer(&e, 40, 6);
    // The visible word "bold" is highlighted; render does not panic projecting
    // a raw-byte match (start=2..6) onto the concealed visible row.
    assert!(row_has_highlight(&buf, 0));
}
```
If `row_has_highlight` / `row_string` helpers don't exist, add small ones in the test module that scan the ratatui `Buffer` cells for a non-default bg / collect a row's chars.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p wordcartel search_highlights highlight_skips_concealed`
Expected: FAIL (no search bar, no highlight).

- [ ] **Step 3: Implement the highlight projection in the row loop**

In the editing-area row loop, the line layout is bound as `(visual_rows, map)` (rename `_map` → `map`). Before the loop, gather the match set + current from the overlay:
```rust
let (hl_matches, hl_current): (&[wordcartel_core::search::Match], Option<wordcartel_core::search::Match>) =
    match editor.search.as_ref() {
        Some(s) => (s.matches(), s.current()),
        None => (&[], None),
    };
```
When building each visual row's spans, **if `!hl_matches.is_empty()`**, build spans from `map.placed` filtered to this visual row instead of from `vr.segs`, applying highlight bg where the glyph's global src overlaps a match. Keep the existing `vr.segs` path verbatim when there is no active search (true no-op):
```rust
let line_off = derive::line_start(&editor.active().document.buffer, l);
let row_index = /* this vr's wrap-row index within the line */;
let spans: Vec<Span> = if hl_matches.is_empty() {
    // EXISTING segs-based path, unchanged.
    build_segs_spans(vr, row_dim)
} else {
    let mut spans = Vec::new();
    if let Some(ref glyph) = vr.prefix_glyph { /* same prefix handling as today */ }
    // One Span per glyph run, split on (style, highlight-kind) change.
    let mut run = String::new();
    let mut run_style = None;
    for p in map.placed.iter().filter(|p| p.row == row_index) {
        let g_from = line_off + p.src.start;
        let g_to = line_off + p.src.end;
        let is_current = hl_current.is_some_and(|m| overlaps(g_from, g_to, m.start, m.end));
        let is_match = hl_matches.iter().any(|m| overlaps(g_from, g_to, m.start, m.end));
        let mut style = if row_dim { RStyle::default().fg(Color::DarkGray) } else { style_to_ratatui(p.style) };
        if is_current { style = style.add_modifier(Modifier::REVERSED); }
        else if is_match { style = style.bg(Color::Yellow).fg(Color::Black); }
        // flush run on style change
        if run_style != Some(style) && !run.is_empty() { spans.push(Span::styled(std::mem::take(&mut run), run_style.unwrap())); }
        run_style = Some(style); run.push_str(&p.text);
    }
    if !run.is_empty() { spans.push(Span::styled(run, run_style.unwrap())); }
    spans
};
```
Add the half-open overlap helper near `row_is_active`:
```rust
pub(crate) fn overlaps(a0: usize, a1: usize, b0: usize, b1: usize) -> bool { a0 < b1 && b0 < a1 }
```
> Note: `row_index` is the position of `vr` within `visual_rows` (track it as the loop enumerates), matching `Placed.row`. `map.placed` for an inactive concealed line omits the `**` markers entirely, so a raw-byte match over `2..6` simply lands on the visible `bold` glyphs — no special-casing.

- [ ] **Step 4: Implement the search bar (status row)**

In the status-row composition, add a branch **before** the minibuffer/prompt branches:
```rust
let (status_text, status_style) = if let Some(ref s) = editor.search {
    (format_search_bar(s), RStyle::default().add_modifier(Modifier::REVERSED))
} else if let Some(ref mb) = editor.minibuffer { /* existing */ }
  else if let Some(ref prompt) = editor.prompt { /* existing */ }
  else { /* existing normal/word-count path */ };
```
```rust
fn format_search_bar(s: &crate::search_overlay::SearchState) -> String {
    use crate::search_overlay::Phase;
    let mode = if matches!(s.mode, wordcartel_core::search::QueryMode::Regex) { " .*" } else { "" };
    let case = match s.case { wordcartel_core::search::CaseMode::Smart => " Aa~",
                              wordcartel_core::search::CaseMode::Sensitive => " Aa",
                              wordcartel_core::search::CaseMode::Insensitive => " aa" };
    let count = if s.error.is_some() { " ?".to_string() }
                else if s.count() == 0 { " no matches".to_string() }
                else { format!(" {}/{}", s.current_ordinal().unwrap_or(0), s.count()) };
    let wrapped = if s.wrapped { " (wrapped)" } else { "" };
    match s.phase {
        Phase::Replace | Phase::Stepping =>
            format!("Find: {}  Replace: {}{}{}{}{}", s.needle, s.template, mode, case, count, wrapped),
        Phase::Find =>
            format!("Find: {}{}{}{}{}", s.needle, mode, case, count, wrapped),
    }
}
```
And in the hardware-cursor section, when `editor.search.is_some()` place the caret on the status row at the focused field's caret column (mirror the minibuffer caret math).

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p wordcartel search_highlights highlight_skips_concealed`
Expected: PASS. Then full `cargo test -p wordcartel` — existing render tests green (inactive search = segs path unchanged).

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/render.rs
git commit -m "feat(search): render — search bar + ColMap-projected highlight layer (current vs other match)"
```

---

## Task 6: Replace-all (single undo unit) + Replace field/Tab/Alt+A

**Files:**
- Modify: `wordcartel/src/commands.rs` (multi-op builder)
- Modify: `wordcartel/src/app.rs` (Tab focus switch, Alt+A replace-all, origin remap)

**Interfaces:**
- Consumes: `editor.search` (`matches`, `matcher`, `needle`, `template`, `mode`, `origin`); `search::expand_replacement`; `editor.apply`; `Transaction::new(cs).with_selection(..)`; `EditKind::Other`; `change::map_pos`.
- Produces:
  ```rust
  // commands.rs
  pub fn build_multi_replace(edits: &[(usize, usize, String)], doc_len: usize)
      -> (wordcartel_core::change::ChangeSet, wordcartel_core::block_tree::Edit);
  ```
  where `edits` is a list of `(from, to, replacement)` in **ascending, non-overlapping** order.

- [ ] **Step 1: Write the failing test (commands.rs)**

```rust
#[test]
fn multi_replace_builds_one_changeset_covering_all() {
    // "aa aa aa" replace all "aa" -> "b": expect "b b b"
    let (cs, edit) = build_multi_replace(
        &[(0, 2, "b".into()), (3, 5, "b".into()), (6, 8, "b".into())], 8);
    let mut tb = wordcartel_core::buffer::TextBuffer::from_str("aa aa aa");
    cs.apply(&mut tb);
    assert_eq!(tb.slice(0..tb.len()), "b b b");
    assert_eq!(edit.range, 0..8); // covering edit spans first.start..last.end
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel multi_replace_builds_one`
Expected: FAIL (`build_multi_replace` not defined).

- [ ] **Step 3: Implement `build_multi_replace`**

In `commands.rs`:
```rust
/// Build ONE ChangeSet performing all `edits` (ascending, non-overlapping
/// (from,to,replacement)) plus ONE covering block_tree::Edit spanning
/// first.start..last.end. Applied as a single editor.apply → one undo unit.
pub fn build_multi_replace(
    edits: &[(usize, usize, String)], doc_len: usize,
) -> (wordcartel_core::change::ChangeSet, wordcartel_core::block_tree::Edit) {
    use wordcartel_core::change::{ChangeSet, Op, Tendril};
    debug_assert!(!edits.is_empty());
    let mut ops = Vec::new();
    let mut pos = 0usize;
    let mut len_after = doc_len;
    for (from, to, text) in edits {
        if *from > pos { ops.push(Op::Retain(from - pos)); }
        if to > from { ops.push(Op::Delete(to - from)); }
        if !text.is_empty() { ops.push(Op::Insert(Tendril::from(text.as_str()))); }
        len_after = len_after - (to - from) + text.len();
        pos = *to;
    }
    if doc_len > pos { ops.push(Op::Retain(doc_len - pos)); }
    let first = edits.first().unwrap().0;
    let last_to = edits.last().unwrap().1;
    // new_len of the covering region = (last_to - first) adjusted by all deltas.
    let delta: isize = edits.iter().map(|(f, t, s)| s.len() as isize - (t - f) as isize).sum();
    let new_len = ((last_to - first) as isize + delta) as usize;
    let cs = ChangeSet { ops, len_before: doc_len, len_after };
    let edit = wordcartel_core::block_tree::Edit { range: first..last_to, new_len };
    (cs, edit)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wordcartel multi_replace_builds_one`
Expected: PASS.

- [ ] **Step 5: Write the failing reduce test (replace-all is one undo unit + origin remap)**

In `app.rs` tests:
```rust
#[test]
fn replace_all_is_one_undo_unit_and_remaps_origin() {
    use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
    let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    let r = |e: &mut Editor, ev| reduce(Msg::Input(ev), e, &reg, &cua_keymap(), &ex, &clk, &tx);
    r(&mut e, press(KeyCode::Char('r'), KeyModifiers::CONTROL));   // open Replace
    for c in "aa".chars() { r(&mut e, press(KeyCode::Char(c), KeyModifiers::NONE)); }
    r(&mut e, press(KeyCode::Tab, KeyModifiers::NONE));            // focus Template
    r(&mut e, press(KeyCode::Char('b'), KeyModifiers::NONE));
    r(&mut e, press(KeyCode::Char('a'), KeyModifiers::ALT));       // Alt+A = Replace All
    assert_eq!(e.active().document.buffer.snapshot().to_string(), "b b b\n");
    let v_after = e.active().document.version;
    assert!(e.active_mut().undo());                                // ONE undo reverts ALL
    assert_eq!(e.active().document.buffer.snapshot().to_string(), "aa aa aa\n");
    let _ = v_after;
}
```

- [ ] **Step 6: Run to verify it fails**

Run: `cargo test -p wordcartel replace_all_is_one_undo_unit`
Expected: FAIL (Tab/Alt+A not handled).

- [ ] **Step 7: Wire Tab + Alt+A in the reduce search branch**

In the `match k.code` of Task 4's search branch, add:
```rust
KeyCode::Tab => {
    if let Some(s) = editor.search.as_mut() {
        s.field = match s.field { crate::search_overlay::Field::Needle => crate::search_overlay::Field::Template,
                                  crate::search_overlay::Field::Template => crate::search_overlay::Field::Needle };
        s.cursor = s.focused_field().len();
    }
}
KeyCode::Char('a') if alt => { search_replace_all(editor, clock); return; }
```
Add the helper:
```rust
fn search_replace_all(editor: &mut Editor, clock: &dyn Clock) {
    search_sync(editor); // ensure cache current
    let plan: Option<(Vec<(usize,usize,String)>, usize, usize)> = editor.search.as_ref().and_then(|s| {
        let m = s.matcher()?; if s.matches().is_empty() { return None; }
        let rope = editor.active().document.buffer.snapshot();
        let edits: Vec<(usize,usize,String)> = s.matches().iter().map(|mm| {
            (mm.start, mm.end, wordcartel_core::search::expand_replacement(&rope, m, mm, &s.template, s.mode))
        }).collect();
        Some((edits, rope.len_bytes(), s.origin))
    });
    let Some((edits, doc_len, origin)) = plan else {
        if let Some(s) = editor.search.as_mut() { /* leave a status */ } editor.status = "No matches".into(); return;
    };
    let n = edits.len();
    let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
    // remap origin through this changeset BEFORE moving it into the transaction
    let new_origin = wordcartel_core::change::map_pos(origin, &cs);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(new_origin));
    editor.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
    if let Some(s) = editor.search.as_mut() { s.origin = new_origin; }
    editor.status = format!("Replaced {n} occurrences");
    editor.search = None; // close after replace-all
    derive::rebuild(editor); crate::nav::ensure_visible(editor);
}
```
> Spec §4.5: confirm `[y/n]` UX. For v1 the plan applies replace-all directly on `Alt+A` and reports the count; a confirm prompt can be layered if the review wants it (note for the reviewer — do not silently add a prompt path the tests don't cover).

- [ ] **Step 8: Run to verify it passes**

Run: `cargo test -p wordcartel replace_all_is_one_undo_unit multi_replace_builds_one`
Expected: PASS. Then full `cargo test -p wordcartel`.

- [ ] **Step 9: Commit**

```bash
git add wordcartel/src/commands.rs wordcartel/src/app.rs
git commit -m "feat(search): replace-all — one composed ChangeSet (single undo unit) + origin remap + Tab/Alt+A"
```

---

## Task 7: Interactive query-replace (stepping)

**Files:**
- Modify: `wordcartel/src/app.rs` (Stepping phase: Alt+Enter to start; y/n/!/q)

**Interfaces:**
- Consumes: Task 6 helpers; `search::find_next` (re-find on mutated rope); `commands::build_range_replace` (single-match `y`) and `build_multi_replace` (`!`); `map_pos`.
- Produces: query-replace stepping behavior per spec §4.6.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn query_replace_steps_yes_no_and_remaps() {
    use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
    let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    let r = |e: &mut Editor, ev| reduce(Msg::Input(ev), e, &reg, &cua_keymap(), &ex, &clk, &tx);
    r(&mut e, press(KeyCode::Char('r'), KeyModifiers::CONTROL));
    for c in "aa".chars() { r(&mut e, press(KeyCode::Char(c), KeyModifiers::NONE)); }
    r(&mut e, press(KeyCode::Tab, KeyModifiers::NONE));
    r(&mut e, press(KeyCode::Char('b'), KeyModifiers::NONE));
    r(&mut e, press(KeyCode::Enter, KeyModifiers::ALT));           // Alt+Enter starts stepping
    assert_eq!(e.search.as_ref().unwrap().phase, crate::search_overlay::Phase::Stepping);
    r(&mut e, press(KeyCode::Char('y'), KeyModifiers::NONE));      // replace #1
    r(&mut e, press(KeyCode::Char('n'), KeyModifiers::NONE));      // skip #2
    r(&mut e, press(KeyCode::Char('y'), KeyModifiers::NONE));      // replace #3
    assert_eq!(e.active().document.buffer.snapshot().to_string(), "b aa b\n");
}

#[test]
fn query_replace_bang_finishes_rest() {
    use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    let mut e = Editor::new_from_text("aa aa aa\n", None, (80, 24));
    let (tx, _rx) = std::sync::mpsc::channel::<Msg>();
    let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
    let press = |code, m| Event::Key(KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE });
    let r = |e: &mut Editor, ev| reduce(Msg::Input(ev), e, &reg, &cua_keymap(), &ex, &clk, &tx);
    r(&mut e, press(KeyCode::Char('r'), KeyModifiers::CONTROL));
    for c in "aa".chars() { r(&mut e, press(KeyCode::Char(c), KeyModifiers::NONE)); }
    r(&mut e, press(KeyCode::Tab, KeyModifiers::NONE));
    r(&mut e, press(KeyCode::Char('b'), KeyModifiers::NONE));
    r(&mut e, press(KeyCode::Enter, KeyModifiers::ALT));
    r(&mut e, press(KeyCode::Char('!'), KeyModifiers::NONE));      // finish all remaining
    assert_eq!(e.active().document.buffer.snapshot().to_string(), "b b b\n");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p wordcartel query_replace_steps query_replace_bang`
Expected: FAIL (Stepping not handled).

- [ ] **Step 3: Implement stepping**

In the search reduce branch, route by phase. When `phase == Stepping`, intercept `y`/`n`/`!`/`q` **before** the text-insert arm:
```rust
if editor.search.as_ref().map(|s| s.phase) == Some(crate::search_overlay::Phase::Stepping) {
    match k.code {
        KeyCode::Char('y') => { search_step_apply(editor, clock); }
        KeyCode::Char('n') => { search_step_skip(editor); }
        KeyCode::Char('!') => { search_step_rest(editor, clock); }
        KeyCode::Char('q') | KeyCode::Esc => { editor.search = None; }
        _ => {}
    }
    return;
}
```
Add `Alt+Enter` to the non-stepping arm to enter stepping:
```rust
KeyCode::Enter if alt => {
    if let Some(s) = editor.search.as_mut() { s.phase = crate::search_overlay::Phase::Stepping; }
    search_sync(editor); // park on first match
}
```
Helpers:
```rust
fn search_step_apply(editor: &mut Editor, clock: &dyn Clock) {
    let plan = editor.search.as_ref().and_then(|s| {
        let m = s.matcher()?; let cur = s.current()?;
        let rope = editor.active().document.buffer.snapshot();
        let text = wordcartel_core::search::expand_replacement(&rope, m, &cur, &s.template, s.mode);
        Some((cur, text, rope.len_bytes(), s.origin))
    });
    let Some((cur, text, doc_len, origin)) = plan else { editor.search = None; return; };
    let (cs, edit) = crate::commands::build_range_replace(cur.start, cur.end, &text, doc_len);
    let new_origin = wordcartel_core::change::map_pos(origin, &cs);
    let caret = cur.start + text.len();
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(caret));
    editor.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
    // Re-find the next match on the MUTATED rope, and remap origin.
    let (rope, version) = { let d = &editor.active().document; (d.buffer.snapshot(), d.version) };
    if let Some(s) = editor.search.as_mut() {
        s.origin = new_origin;
        s.cache_invalidate();                 // force recompute against mutated rope
        s.recompute(&rope, version);
        s.set_current_at_or_after(caret);     // park on next match at/after the just-edited spot
    }
    search_pin(editor);
    if editor.search.as_ref().is_some_and(|s| s.current().is_none()) { editor.search = None; } // done
}
fn search_step_skip(editor: &mut Editor) {
    if let Some(s) = editor.search.as_mut() { s.next(); }
    search_pin(editor);
    if editor.search.as_ref().is_some_and(|s| s.wrapped) { editor.search = None; } // walked off the end
}
fn search_step_rest(editor: &mut Editor, clock: &dyn Clock) {
    // Replace current + all remaining (from current.start onward) as one unit.
    let plan = editor.search.as_ref().and_then(|s| {
        let m = s.matcher()?; let cur = s.current()?;
        let rope = editor.active().document.buffer.snapshot();
        let edits: Vec<(usize,usize,String)> = s.matches().iter().filter(|mm| mm.start >= cur.start)
            .map(|mm| (mm.start, mm.end, wordcartel_core::search::expand_replacement(&rope, m, mm, &s.template, s.mode)))
            .collect();
        Some((edits, rope.len_bytes()))
    });
    let Some((edits, doc_len)) = plan else { editor.search = None; return; };
    if edits.is_empty() { editor.search = None; return; }
    let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
    let txn = wordcartel_core::history::Transaction::new(cs)
        .with_selection(wordcartel_core::selection::Selection::single(edits[0].0));
    editor.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
    editor.search = None;
    derive::rebuild(editor); crate::nav::ensure_visible(editor);
}
fn search_pin(editor: &mut Editor) {
    if let Some(m) = editor.search.as_ref().and_then(|s| s.current()) {
        editor.active_mut().document.selection = wordcartel_core::selection::Selection::range(m.start, m.end);
        derive::rebuild(editor); crate::nav::ensure_visible(editor);
    }
}
```
Add to `SearchState` (search_overlay.rs) the two small helpers used above:
```rust
pub fn cache_invalidate(&mut self) { self.cache_sig = None; }
pub fn set_current_at_or_after(&mut self, off: usize) { self.cur_idx = self.first_at_or_after_strict(off); }
fn first_at_or_after_strict(&self, off: usize) -> Option<usize> {
    self.matches.iter().position(|m| m.start >= off) // None when past the last match → stepping ends
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p wordcartel query_replace_steps query_replace_bang`
Expected: PASS. Then full `cargo test -p wordcartel` + `cargo test -p wordcartel-core`.

- [ ] **Step 5: Commit**

```bash
git add wordcartel/src/app.rs wordcartel/src/search_overlay.rs
git commit -m "feat(search): interactive query-replace — y/n/!/q stepping, re-find on mutated rope, origin remap"
```

---

## Self-Review (completed by plan author)

**Spec coverage:**
- §3.1 core API → Tasks 1–2. §3.2 SearchState + cache → Task 3. §3.3 overlay XOR → Task 3 (open_search + every open_*). §4.1 open → Task 4. §4.2 incremental find → Task 4. §4.3 navigate → Task 4. §4.4 highlight via ColMap → Task 5. §4.5 replace-all → Task 6. §4.6 query-replace → Task 7. §4.7 remapping (origin via map_pos; per-match re-find) → Tasks 6–7. §4.8 Esc-to-origin + buffer-swap close → Task 4. §5 keys/bar → Tasks 4–5. §6 perf (viewport highlight, version-keyed cache, full-scan count) → Tasks 3 & 5. §7 semantics (literal escape, smart-case in compile, zero-width boundary, captures) → Tasks 1–2. §8 error handling (invalid regex, empty needle, 0 matches, $N OOR) → Tasks 1–6. §9 tests → every task. §9.3 build gate → Task 1.
- **Known deviation flagged for the user/review:** spec §3.2 said "stores no match list"; this plan uses a **version-keyed match cache** for per-frame responsiveness (Global Constraints) — a deliberate refinement, not a gap.
- **Confirm-on-replace-all:** spec §4.5 shows a `[y/n]` confirm; Task 6 applies directly on `Alt+A` and flags this for the reviewer rather than adding an untested prompt path.

**Type consistency:** `Match{start,end}`, `QueryMode`, `CaseMode`, `Matcher` used identically core↔shell; `SearchState` field/method names match across Tasks 3–7 (`matches()`, `matcher()`, `current()`, `cache_invalidate()`, `set_current_at_or_after()`); `build_multi_replace`/`build_range_replace` signatures consistent commands↔app.

**Placeholder scan:** the two `next_from` / `expand_replacement` bodies carry explicit "adapt to the resolved crate version, keep the contract+tests" implementer notes (the regex-cursor surface is version-pinned at the Task-1 gate) — these are bounded, not open-ended TODOs.
