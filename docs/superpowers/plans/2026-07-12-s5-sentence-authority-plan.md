# S5 — Sentence authority: implementation PLAN

**Status:** PLAN (implementation-grade), authored 2026-07-12. Drives subagent-driven TDD (fresh
implementer per task: failing test → impl → green → commit; reviewer per task). Entering the Codex
plan gate.

**Authority:** the Codex-clean spec — `docs/superpowers/specs/2026-07-12-s5-sentence-authority-design.md`
(committed on branch `effort-s5-sentence-authority`). Section references below (§3, §4, …) point at
that spec. Where the spec and this plan agree, the spec governs the *why* and this plan governs the
*how*.

**Grounding:** every snippet, signature, and anchor below was re-verified against the real source
2026-07-12, and the entire §4 post-pass algorithm + the two motion kernels were **implemented and
executed against all spec fixtures** in a scratch crate before being written here (all 28 fixtures
green, alloc-free iterator form). Anchors are symbol-anchored; if a line number has drifted, locate
by the named symbol.

---

## Global Constraints (reviewer lens — applies to EVERY task)

1. **The Codex-clean spec is the authority.** A finding that contradicts the spec is a human
   decision, not a silent fix (see the FLAGS section at the end — one design-level item was forced
   during implementation and needs ratification).
2. **`wordcartel-core` is pure.** `#![forbid(unsafe_code)]`, **NO `repar` dependency**, no shell
   types. The differential suite (Task 5) that needs repar lives in the **SHELL** crate
   (`wordcartel/tests/`). Core additions depend only on `unicode-segmentation` (already a dep,
   `wordcartel-core/Cargo.toml:12`) and `std`.
3. **House style — hand-formatted.** Do **NOT** run `cargo fmt` (no `rustfmt.toml`; it would reflow
   the tree). Match the dense neighbor style by hand: 4-space indent, aligned `match` arms where the
   neighbors align, `—` em-dashes in prose comments (never `--`). **No emoji anywhere except the
   multibyte TEST fixtures** (`é` / `中` / `🙂`). Doc-comment every new public item (params /
   returns; `# Examples` for the non-obvious ones per the spec).
4. **Merge GATEs (each task must leave ALL green — the tree compiles+passes after every task):**
   - `cargo test --workspace` green (core lib + oracle, shell lib, shell integration tests).
   - `cargo build` and `cargo test --no-run` warning-free for touched crates.
   - `cargo clippy --workspace --all-targets` clean (`[workspace.lints.clippy] all = "deny"`).
   - `wordcartel/tests/module_budgets.rs` 5/5.
   - Command-surface invariants (Tasks 3–4): `palette_is_exhaustive_over_the_registry`
     (`palette.rs:255`), `hints_reresolve_on_preset_switch` (`keymap.rs:1087`),
     `custom_bind_surfaces_in_menu_and_palette` (`menu.rs:435`), keymap build warns empty.
5. **Allocation budget (§4.8 — mandatory).** R1–R3 and the `sentence_spans` fold are
   **allocation-free**; the production span iterator allocates **nothing** (no `Vec` of segments, no
   `String`). Reason: `gather_row_ctx` (`render.rs:494-506`) calls `sentence_bounds` **per frame**
   under Focus=Sentence and already allocates one `String` via `buf.slice(ps..pe)` — do not add a
   second. Test code (Tasks 5/6 and `#[cfg(test)]`) may allocate freely.
6. **`clippy::too_many_lines` threshold = 100** (`clippy.toml`). The post-pass is factored into
   small free helper fns (`content_end`, `ends_terminated`, `semantic_hard_break`, `r1_merge`,
   `r2_merge`, `r3_merge`, `r4_run`, `merges`) plus a ~30-line `Iterator::next`; **no function
   exceeds 100 lines, so no `#[allow]` is needed.** If a reviewer's refactor pushes `next` over,
   prefer extracting the R4 block into a helper over adding an allow.
7. **Do NOT run `cargo fmt`, do NOT hand-edit `BACKLOG.md`/`backlog.toml`** (this effort ships via
   the normal merge; backlog status is flipped separately when it merges).
8. **Commit trailers (verbatim) on every commit** — append after the one-line subject each task
   specifies:
   ```
   Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
   Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG
   ```

**Anchor table (verified 2026-07-12):**

| Symbol | Location |
|---|---|
| `sentence_bounds` (rewritten) | `wordcartel-core/src/textobj.rs:43` |
| `textobj` test module | `wordcartel-core/src/textobj.rs:56` (`sentence_bounds_basic` @ 90-96, `empty_window_is_safe` @ 97-103) |
| `Dir` enum (add 2 variants) | `wordcartel/src/commands.rs:35` |
| `Move` arm exhaustive `match dir` (add 2 arms) — the ONLY exhaustive match over `commands::Dir` | `wordcartel/src/commands.rs:256-273` |
| `scope_range_at` `Scope::Sentence` arm | `wordcartel/src/commands.rs:212-216` |
| `ExpandSelection` ladder | `wordcartel/src/commands.rs:439-456` (containment test line 447) |
| `move_word_right` / `move_word_left` (templates) | `wordcartel/src/nav.rs:847` / `877` |
| `head` / `paragraph_range_at` / `next_paragraph_start` / `prev_paragraph_start` | `nav.rs:40` / `655` / `704` / `709` |
| word-motion registry rows (mirror) | `wordcartel/src/registry.rs:185-191`; `register` fn @ `111`; `select_sentence` @ `330` |
| CUA keymap table `static CUA` | `wordcartel/src/keymap.rs:257` (jump-ring `alt-left/right` @ 326-327) |
| WordStar table `static WORDSTAR` | `wordcartel/src/keymap.rs:375` |
| `run_transform` / `TransformKind` / `FIXUPS_STACK` (shell) | `wordcartel/src/transform.rs:326` / `8` / `323` |
| `gather_row_ctx` focus path | `wordcartel/src/render.rs:494-506` |
| e2e `Harness` (editor `Rc<RefCell<Editor>>`, `term: TestBackend`, `underlined_cols` helper) | `wordcartel/src/e2e.rs:54-64`, `258-263` |
| `view_opts.focus` / `.focus_granularity` (`ViewConfig`) | `wordcartel/src/config.rs:157-158`; enum `FocusGranularity{Paragraph,Sentence}` @ `94` |

---

## Task list

| # | Title | Crate | Seam it touches |
|---|---|---|---|
| 1 | Content-only `sentence_bounds` + `sentence_spans` 4-rule post-pass + consts + the pin flip | wordcartel-core | pure core (valid-by-construction); the deliberate contract flip |
| 2 | Motion kernels `prev_sentence_start` + `next_sentence_end` | wordcartel-core | pure core |
| 3 | `Dir::SentenceLeft/Right` + `Move` arms + `nav::move_sentence_left/right` + motion & ladder tests | wordcartel | exhaustive `Move` match (registration seam) + nav module |
| 4 | 4 registry rows + CUA `Alt+a/e` keybindings + dispatch/resolution tests | wordcartel | registry registration seam + CUA keymap table (command surface) |
| 5 | Differential suite (equality corpus + divergence ledger) | wordcartel | test-only integration test |
| 6 | Focus-mode §9 behavior-change test | wordcartel | test-only (e2e) |

Dependency order: 1 → 2 → 3 → 4, with 5 and 6 depending only on Task 1 (may land any time after 1;
listed last). Each task is independently green.

---

## Task 1 — Core: content-only `sentence_bounds` + `sentence_spans` 4-rule post-pass

**Crate/file:** `wordcartel-core/src/textobj.rs` only.
**Command-surface conformance:** N/A — does not touch the command surface.
**Depends on:** nothing.

### 1.1 What lands

New public API and its private machinery, plus the rewrite of `sentence_bounds` into a thin
consumer, plus the **deliberate visible flip** of the `sentence_bounds_basic` pin — **all in one
commit** (the flip must never precede or lag the implementation, §3.3).

### 1.2 Failing tests first (write these, watch them fail against the OLD code)

Add to the existing `#[cfg(test)] mod tests` (`textobj.rs:56`). Rewrite `sentence_bounds_basic`
in place; add the rest. Assert **slice text** where a span is checked.

```rust
    // --- S5: content-only sentence_bounds (deliberate contract change) ---
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
```

### 1.3 Implementation (complete — verified against all the above)

Insert **above** `sentence_bounds` (`textobj.rs:42`) the consts + helpers + iterator, then replace
`sentence_bounds`'s body. Keep `use unicode_segmentation::UnicodeSegmentation;` (line 5) and add
`USentenceBoundIndices` to it.

```rust
use unicode_segmentation::{UnicodeSegmentation, USentenceBoundIndices};
```

```rust
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
    text[..nl].ends_with("  ") || text[..nl].ends_with('\\')
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
        loop {
            let Some((mut n_start, n_raw)) = self.pull() else { break };
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
```

Then replace `sentence_bounds` (`textobj.rs:43-54`) with the thin consumer:

```rust
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
```

### 1.4 Verify green

`cargo test -p wordcartel-core` (all new tests + the untouched `word_bounds`/`next_word_start`
tests pass); `cargo test --workspace` (the shell still passes — its `expand_then_shrink_round_trips`
uses `starts_with("One two.")`, and no shell test pins the old trailing-space span, verified);
`cargo clippy -p wordcartel-core --all-targets`; `cargo test --doc -p wordcartel-core` (the
`# Examples` block runs). Confirm `sentence_bounds_basic` now asserts `(0, 8)`.

### 1.5 Commit

`feat(core): content-only sentence_bounds via a 4-rule UAX-29 post-pass (S5)`

---

## Task 2 — Core: motion kernels `prev_sentence_start` + `next_sentence_end`

**Crate/file:** `wordcartel-core/src/textobj.rs` only.
**Command-surface conformance:** N/A — does not touch the command surface.
**Depends on:** Task 1 (`sentence_spans`).

### 2.1 Failing tests first

```rust
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
```

### 2.2 Implementation (complete — verified)

Add beside `sentence_spans`:

```rust
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
```

### 2.3 Verify green

`cargo test -p wordcartel-core`; `cargo clippy -p wordcartel-core --all-targets`;
`cargo test --doc -p wordcartel-core`.

### 2.4 Commit

`feat(core): prev_sentence_start / next_sentence_end motion kernels (S5)`

---

## Task 3 — Shell: `Dir` sentence variants + `Move` arms + nav fns + motion/ladder tests

**Crate/files:** `wordcartel/src/commands.rs`, `wordcartel/src/nav.rs`.
**Command-surface conformance:** touches the `Dir`/`Move` **dispatch** seam (adds motion
primitives) but adds **no registry rows or keybindings** here — the commands are reachable only via
`Command::Move { dir: Dir::Sentence.., .. }` in tests until Task 4. No palette/menu/hint surface
changes in this task; the invariant gates are unaffected. (Rationale for splitting 3/4: a `Dir`
variant without a registry row reds no gate; a registry row without a palette entry would — so rows
+ keymap land together in Task 4.)
**Depends on:** Task 2.

### 3.1 Failing tests first (in `commands.rs` `#[cfg(test)] mod tests`, near `doc_start_and_end` @ 1101)

```rust
    #[test]
    fn sentence_motion_start_and_end() {
        // spans: "One two." (0,8), "Three four." (9,20)
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        set_caret(&mut e, 12); derive::rebuild(&mut e);          // inside "Three four."
        run(Command::Move { dir: Dir::SentenceLeft, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 9);                             // start of current sentence
        run(Command::Move { dir: Dir::SentenceLeft, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 0);                             // idempotent-safe → previous
        run(Command::Move { dir: Dir::SentenceRight, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 8);                             // end of current CONTENT
        run(Command::Move { dir: Dir::SentenceRight, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 20);                            // → next content end
    }
    #[test]
    fn sentence_motion_crosses_blocks_both_directions() {
        // "One. Two." = 0..9 (spans (0,4),(5,9)); "Three. Four." = 11..23 (spans (11,17),(18,23)).
        // Core offsets executed-verified 2026-07-12: prev_sentence_start("Three. Four.",0)=None →
        // cross → prev_sentence_start("One. Two.",len)=Some(5); next_sentence_end("One. Two.",8)=9,
        // next_sentence_end("Three. Four.",0)=6 (→ 11+6=17).
        let mut e = Editor::new_from_text("One. Two.\n\nThree. Four.\n", None, (80, 24));
        // RIGHTWARD: from block 1 crosses to block 2's FIRST content end.
        set_caret(&mut e, 8); derive::rebuild(&mut e);            // in "Two."
        run(Command::Move { dir: Dir::SentenceRight, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 9);                             // end of "Two."
        run(Command::Move { dir: Dir::SentenceRight, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 17);                            // crosses to end of "Three."
        // LEFTWARD: from block 2's FIRST sentence start crosses to block 1's LAST sentence start.
        set_caret(&mut e, 11); derive::rebuild(&mut e);          // AT start of "Three." (block 2)
        run(Command::Move { dir: Dir::SentenceLeft, extend: false }, &mut e, &TestClock(0));
        assert_eq!(nav::head(&e), 5);                             // crosses to start of "Two." (block 1's LAST)
    }
    #[test]
    fn sentence_motion_extends_selection() {
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        set_caret(&mut e, 0); derive::rebuild(&mut e);
        run(Command::Move { dir: Dir::SentenceRight, extend: true }, &mut e, &TestClock(0));
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (0, 8));               // anchor kept, head → 8
    }
    #[test]
    fn expand_ladder_sentence_rung_survives_single_sentence_paragraph() {
        // §3.4.3 regression: with content-only spans, Sentence (0,8) ⊂ Paragraph (0,9),
        // so the Sentence rung no longer collapses into Paragraph.
        let mut e = Editor::new_from_text("One two.\n", None, (80, 24));
        set_caret(&mut e, 1); derive::rebuild(&mut e);            // inside "One"
        run(Command::ExpandSelection, &mut e, &TestClock(0));     // → Word "One"
        run(Command::ExpandSelection, &mut e, &TestClock(0));     // → Sentence "One two."
        let s = e.active().document.selection.primary();
        assert_eq!((s.from(), s.to()), (0, 8));
        assert_eq!(e.active().document.buffer.slice(s.from()..s.to()), "One two.");
    }
```

> Cross-block offsets in `sentence_motion_crosses_blocks_both_directions` are derived from the
> fixture and the executed-verified core spans (block 2 `"Three. Four."` starts after
> `"One. Two.\n\n"` = byte 11; rightward `"Three."` content end = 11+6 = 17; leftward from byte 11
> crosses to block 1's last sentence start = 5). If the paragraph model yields a different block
> start, the implementer adjusts each expected value to the observed one and notes it — the pinned
> *behavior* is "rightward crosses to the next block's FIRST sentence end; leftward crosses to the
> previous block's LAST sentence start" (spec T-10). The leftward primitive is `prev_paragraph_start`
> (`nav.rs:709`), already used by `move_sentence_left`.

### 3.2 Implementation

**`commands.rs`** — add two variants to `Dir` (`commands.rs:35`, after `WordRight`):

```rust
    WordLeft,
    WordRight,
    SentenceLeft,
    SentenceRight,
```

Add two arms to the exhaustive `match dir` (`commands.rs:256-273`, after the `WordRight` arm) —
the compiler forces exactly these two (this is the only exhaustive `Dir` match, verified):

```rust
                Dir::WordLeft      => nav::move_word_left(editor),
                Dir::WordRight     => nav::move_word_right(editor),
                Dir::SentenceLeft  => nav::move_sentence_left(editor),
                Dir::SentenceRight => nav::move_sentence_right(editor),
```

**`nav.rs`** — add the two fns beside the word templates (`nav.rs:847-903`), mirroring them exactly
(swap `next_word_start` → `next_sentence_end`, `prev_word_start` → `prev_sentence_start`):

```rust
/// Move to the start of the current sentence, or of the previous one when already
/// there (Emacs M-a), crossing block boundaries (skipping gaps).
pub fn move_sentence_left(editor: &mut Editor) -> usize {
    let h = head(editor);
    let new = {
        let buf = &editor.active().document.buffer;
        let blocks = editor.active().document.blocks();
        let (wstart, wend) = paragraph_range_at(blocks, buf, h);
        let window = buf.slice(wstart..wend);
        let rel = h.saturating_sub(wstart);
        match wordcartel_core::textobj::prev_sentence_start(&window, rel) {
            Some(r) => wstart + r,
            None if wstart > 0 => {
                let pps = prev_paragraph_start(blocks, buf, wstart);
                let prev_end = paragraph_range_at(blocks, buf, pps).1;
                let ptext = buf.slice(pps..prev_end);
                wordcartel_core::textobj::prev_sentence_start(&ptext, ptext.len())
                    .map(|r| pps + r)
                    .unwrap_or(pps)
            }
            None => 0,
        }
    };
    editor.active_mut().desired_col = None;
    new
}

/// Move to the end of the current sentence's content, or of the next one when already
/// there (Emacs M-e), crossing block boundaries (skipping gaps).
pub fn move_sentence_right(editor: &mut Editor) -> usize {
    let h = head(editor);
    let new = {
        let buf = &editor.active().document.buffer;
        let blocks = editor.active().document.blocks();
        let (wstart, wend) = paragraph_range_at(blocks, buf, h);
        let window = buf.slice(wstart..wend);
        let rel = h.saturating_sub(wstart);
        match wordcartel_core::textobj::next_sentence_end(&window, rel) {
            Some(r) => wstart + r,
            None => {
                let nps = next_paragraph_start(blocks, buf, wend);
                if nps >= buf.len() {
                    buf.len()
                } else {
                    let next_end = paragraph_range_at(blocks, buf, nps).1;
                    let ntext = buf.slice(nps..next_end);
                    wordcartel_core::textobj::next_sentence_end(&ntext, 0)
                        .map(|r| nps + r)
                        .unwrap_or(nps)
                }
            }
        }
    };
    editor.active_mut().desired_col = None;
    new
}
```

### 3.3 Verify green

`cargo test -p wordcartel` (the four new tests + all existing motion/expand tests);
`cargo clippy -p wordcartel --all-targets` (the compiler-forced arms make the `match` exhaustive
with no `_`); `cargo test --workspace`; module budgets 5/5.

### 3.4 Commit

`feat(nav): sentence motions (Dir::Sentence{Left,Right}, Emacs M-a/M-e) (S5)`

---

## Task 4 — Shell: registry rows + CUA `Alt+a/e` keybindings

**Crate/files:** `wordcartel/src/registry.rs`, `wordcartel/src/keymap.rs`.
**Command-surface conformance (per the contract):** **Law 3** — the four rows appear in the palette
automatically (gated by `palette_is_exhaustive_over_the_registry`). **Law 7** — `Alt+a`/`Alt+e` (and
the shifted pair) hint in CUA, none in WordStar; re-resolution gated by
`hints_reresolve_on_preset_switch` + `custom_bind_surfaces_in_menu_and_palette`. **Law 4** N/A —
all rows `menu: None` (palette-only, the word-motion precedent). **Law 2** N/A — no persisted
option. **Law 10** — all four commands nullary ✓. **No contract amendment.** Rows and keymap rows
land in ONE commit so the palette-exhaustive and keymap-warns-empty gates never see a half-state.
**Depends on:** Task 3 (`Dir::Sentence*` variants must exist).

### 4.1 Failing tests first

In `registry.rs` `#[cfg(test)]` — spec T-11 requires the handlers to actually RUN, so DISPATCH each
of the four via `Registry::dispatch` and assert the caret/selection effect (not just name lookup).
The palette-completeness gate at `palette.rs:255` separately guarantees the rows appear in the
palette:

```rust
    #[test]
    fn sentence_motion_commands_dispatch_and_take_effect() {
        // "One two. Three four." spans: (0,8), (9,20).
        let reg = Registry::builtins();
        let ex = InlineExecutor::default();
        let clk = Z;
        let (tx, _rx) = std::sync::mpsc::channel();
        // Dispatch `id` against editor `e` with the caret preset to `caret`; return the new head
        // (and leave `e`'s selection for the caller to inspect).
        let dispatch = |e: &mut Editor, id: &'static str| {
            let mut ctx = Ctx { editor: e, clock: &clk, executor: &ex, msg_tx: tx.clone() };
            reg.dispatch(CommandId(id), &mut ctx)
        };
        let head = |e: &Editor| e.active().document.selection.primary().head;

        // sentence_left: caret in "Three four." → start of that sentence (9).
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(12);
        crate::derive::rebuild(&mut e);
        assert_eq!(dispatch(&mut e, "sentence_left"), CommandResult::Handled);
        assert_eq!(head(&e), 9);

        // sentence_right: caret at 0 → content end of first sentence (8).
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        crate::derive::rebuild(&mut e);
        assert_eq!(dispatch(&mut e, "sentence_right"), CommandResult::Handled);
        assert_eq!(head(&e), 8);

        // select_sentence_right: extends from anchor 0 → selection (0,8).
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        crate::derive::rebuild(&mut e);
        assert_eq!(dispatch(&mut e, "select_sentence_right"), CommandResult::Handled);
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (0, 8));

        // select_sentence_left: caret in "Three four." extends back to that sentence's start (9,12).
        let mut e = Editor::new_from_text("One two. Three four.\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(12);
        crate::derive::rebuild(&mut e);
        assert_eq!(dispatch(&mut e, "select_sentence_left"), CommandResult::Handled);
        let sel = e.active().document.selection.primary();
        assert_eq!((sel.from(), sel.to()), (9, 12));
    }
```

> Grounded against existing dispatch tests (`registry.rs:1037-1051` `dispatch_save_id_runs_save_handler`
> and `:1084-1091`): `Registry::dispatch(&self, CommandId, &mut Ctx) -> CommandResult`
> (`registry.rs:756`) runs a builtin's handler SYNCHRONOUSLY (`HandlerKind::Builtin(h) => h(ctx)`,
> which calls `commands::run` and mutates `ctx.editor` in place — so caret assertions right after the
> call are valid). `Ctx { editor, clock, executor, msg_tx }` (`registry.rs:26-31`) is built exactly
> as those tests build it: `struct Z; impl Clock for Z { fn now_ms(&self)->u64 {0} }`,
> `InlineExecutor::default()` (test-module imports at `registry.rs:983-988`), and a
> `std::sync::mpsc::channel()`; `msg_tx` is an owned `Sender`, so the closure passes `tx.clone()` per
> call. `CommandResult` is already in the test module's scope (registry.rs:9 `use crate::commands::{…,
> CommandResult, …}` + the test module's `use super::*`); `derive` is NOT imported at registry top,
> so the test qualifies it as `crate::derive::rebuild`. The caret/selection targets
> (9 / 8 / (0,8) / (9,12)) are the executed-verified core spans of §4.9.

In `keymap.rs` `#[cfg(test)]` (mirror `cua_alt_b_promotes` / the CUA alt-plane test at
`keymap.rs` ~896 and `close_buffer_is_unbound_in_both_presets_by_design` @ 1056):

```rust
    #[test]
    fn sentence_motions_bound_in_cua_unbound_in_wordstar() {
        let reg = Registry::builtins();
        let seq = |s: &str| parse_seq(s).unwrap();
        let (cua, warns) = build_keymap(
            &crate::config::KeymapConfig { preset: "cua".into(), patches: vec![] }, &reg);
        assert!(warns.is_empty(), "cua warns: {warns:?}");
        assert!(matches!(cua.resolve(&seq("alt-a")), Resolution::Command(CommandId("sentence_left"))));
        assert!(matches!(cua.resolve(&seq("alt-e")), Resolution::Command(CommandId("sentence_right"))));
        assert!(matches!(cua.resolve(&seq("alt-shift-a")), Resolution::Command(CommandId("select_sentence_left"))));
        assert!(matches!(cua.resolve(&seq("alt-shift-e")), Resolution::Command(CommandId("select_sentence_right"))));
        let (ws, warns) = build_keymap(
            &crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] }, &reg);
        assert!(warns.is_empty(), "wordstar warns: {warns:?}");
        // Deliberately unbound in WordStar (law 7: palette-only, no hint). ALL FOUR chords must
        // resolve to NONE of the four sentence commands (spec T-12).
        for chord in ["alt-a", "alt-e", "alt-shift-a", "alt-shift-e"] {
            assert!(!matches!(ws.resolve(&seq(chord)),
                Resolution::Command(CommandId(
                    "sentence_left" | "sentence_right"
                    | "select_sentence_left" | "select_sentence_right"))),
                "WordStar must not bind {chord} to any sentence motion (law 7: palette-only, no hint)");
        }
    }
```

> Grounded names (verified): `build_keymap(&KeymapConfig, &Registry) -> (KeyTrie, Vec<String>)`
> (`keymap.rs:491`), `parse_seq(&str) -> Option<..>` (`keymap.rs:109`), `enum Resolution`
> (`keymap.rs:148`), and the `Resolution::Command(CommandId("literal"))` pattern for builtins is
> proven by the existing CUA alt-plane tests (`keymap.rs:727`, `~896-916`). `Registry::builtins()`
> per Task 4.1.

### 4.2 Implementation

**`registry.rs`** — add after the word-selecting rows (`registry.rs:191`), mirroring their shape
(`register` @ 111; `menu: None`):

```rust
        // Sentence motions (S5, Emacs M-a/M-e) — palette-only (menu: None).
        r.register("sentence_left",  "Move Sentence Left",  None, |c| run(c, Command::Move { dir: Dir::SentenceLeft,  extend: false }));
        r.register("sentence_right", "Move Sentence Right", None, |c| run(c, Command::Move { dir: Dir::SentenceRight, extend: false }));

        // Sentence selecting motions (extend) — palette-only (menu: None).
        r.register("select_sentence_left",  "Select Sentence Left",  None, |c| run(c, Command::Move { dir: Dir::SentenceLeft,  extend: true }));
        r.register("select_sentence_right", "Select Sentence Right", None, |c| run(c, Command::Move { dir: Dir::SentenceRight, extend: true }));
```

**`keymap.rs`** — add to `static CUA` (`keymap.rs:257`), grouped with the other alt-plane rows
(e.g. after the jump-ring rows or the editing group; place near the navigation rows for
readability):

```rust
    // Sentence motions (S5) — Emacs M-a / M-e.
    ("alt-a",       "sentence_left"),
    ("alt-e",       "sentence_right"),
    ("alt-shift-a", "select_sentence_left"),
    ("alt-shift-e", "select_sentence_right"),
```

**`WORDSTAR`** — no rows added (deliberately unbound).

### 4.3 Verify green

`cargo test -p wordcartel` — the two new tests **and** the existing invariant gates
`palette_is_exhaustive_over_the_registry`, `hints_reresolve_on_preset_switch`,
`custom_bind_surfaces_in_menu_and_palette` (they now cover the four rows and must stay green);
`cargo clippy -p wordcartel --all-targets`; `cargo test --workspace`.

### 4.4 Commit

`feat(commands): register sentence motions + CUA Alt+a/Alt+e bindings (S5)`

---

## Task 5 — Shell: differential suite (two corpora — repar ventilate + golden rules)

**Crate/file:** new `wordcartel/tests/sentence_differential.rs`.
**Command-surface conformance:** N/A — test-only.
**Depends on:** Task 1 (`sentence_spans`); the existing `run_transform`.

Two corpora, each split into an equality set (`assert_eq!`) and an accepted-divergence ledger
(`assert_ne!` + reason): (a) vs **repar ventilate** (spec §6.1–6.4, L1–L5); (b) vs
**pragmatic_segmenter's English Golden Rules** (spec §6.5, 34 equality / 18 divergence). Four test
fns total.

### 5.1 The test (there is no "impl" — it IS the deliverable; write it and it must pass)

Drive repar via the shell's `run_transform` (§6.1). Both crates are importable from an integration
test: `wordcartel_core` and `wordcartel` are dependencies of `wordcartel`'s own test target.

```rust
//! S5 differential suite — pins our detector's relationship to repar's ventilate.
//! Equality corpus asserts identical word GROUPINGS (both merge OR both break); the
//! ledger asserts KNOWN divergences with a reason so a vanishing divergence also fails.
//! repar is driven through the shell's own ventilate wrapper — the product's ventilate.

use wordcartel::transform::{run_transform, TransformKind};
use wordcartel_core::textobj::sentence_spans;

const W: u32 = 72; // ventilate is width-agnostic (driver.rs:209); any width is inert.

/// Word groups from our detector: each sentence span → its whitespace words, with
/// `markers` tokens (e.g. `">"`, `"-"`) filtered out on both sides.
fn our_groups(para: &str, markers: &[&str]) -> Vec<Vec<String>> {
    sentence_spans(para)
        .map(|(f, t)| para[f..t].split_whitespace()
            .filter(|w| !markers.contains(w))
            .map(str::to_string).collect())
        .collect()
}
/// Word groups from repar ventilate: each non-marker-only output line → its words.
fn repar_groups(para: &str, markers: &[&str]) -> Vec<Vec<String>> {
    let out = run_transform(TransformKind::Ventilate, para, W).expect("ventilate");
    out.lines()
        .filter(|l| !l.split_whitespace().all(|w| markers.contains(&w)))
        .map(|l| l.split_whitespace()
            .filter(|w| !markers.contains(w))
            .map(str::to_string).collect())
        .collect()
}

#[test]
fn equality_corpus_agrees_with_ventilate() {
    // (input, markers) — each verified against live repar ventilate 2026-07-12.
    let cases: &[(&str, &[&str])] = &[
        ("Dr. Smith arrived. He was late.", &[]),
        ("See fig. 2 for details. Then leave.", &[]),
        ("Kramer vs. Wade was long. He read it.", &[]),
        ("See cf. Smith 2001. He agreed.", &[]),
        ("Smith et al. Wrote it.", &[]),
        ("St. Louis is big. He knows.", &[]),
        ("J. R. R. Tolkien wrote it. He was English.", &[]),
        ("Q.E.D. Next problem.", &[]),                    // both BREAK
        ("We met at 10 a.m. and left.", &[]),             // both one group
        ("The U.S.A. is large. It grew.", &[]),
        // The R2 proof: same content, hard-wrapped, must group identically.
        ("The committee met on Tuesday and the chair insisted on a vote. Then we left.", &[]),
        ("The committee met on Tuesday and the\nchair insisted on a vote. Then we left.", &[]),
        // Blockquote + list-item prefix re-emission (markers filtered both sides).
        ("> The committee met and the\n> chair voted. Then we left.", &[">"]),
        ("- Buy milk. Then rest.", &["-"]),
        // Multibyte with ASCII terminators.
        ("été fini. Then done.", &[]),
    ];
    for (input, markers) in cases {
        assert_eq!(our_groups(input, markers), repar_groups(input, markers),
            "equality corpus mismatch for {input:?}");
    }
}

#[test]
fn divergence_ledger() {
    // Each entry: ours ≠ repar, with a reason. assert_ne so a vanishing divergence fails.
    // L1 — the colon: repar terminal_chars default ".?!:" (options.rs:151); UAX has no ':'.
    assert_ne!(our_groups("Note: This is fine.", &[]), repar_groups("Note: This is fine.", &[]),
        "L1 colon: repar breaks at ':' (terminal_chars \".?!:\"); UAX-29 does not");
    // L2 — ideographic full stop: ours breaks at 。, repar's terminal set is ASCII.
    assert_ne!(our_groups("中文。Then done.", &[]), repar_groups("中文。Then done.", &[]),
        "L2 CJK: ours honors 。 as a terminator; repar's ASCII terminal set does not");
    // L3 — name prefixes mt/ft: ours merges (class 1), repar's list lacks them → breaks.
    assert_ne!(our_groups("I saw Mt. Fuji. It was tall.", &[]),
        repar_groups("I saw Mt. Fuji. It was tall.", &[]),
        "L3 mt/ft: ours R1-merges name prefixes repar's stop-list lacks");
    // L4 — class-2 suffix / dropped 'no': ours breaks on the capital, repar always merges.
    assert_ne!(our_groups("Acme Co. Then he quit.", &[]),
        repar_groups("Acme Co. Then he quit.", &[]),
        "L4 class-2: ours breaks after 'Co.'+capital; repar's flat stop-list merges");
    assert_ne!(our_groups("The answer was no. Then we left.", &[]),
        repar_groups("The answer was no. Then we left.", &[]),
        "L4 dropped 'no': ours breaks; repar merges (its most damaging entry)");
    // L5 — Markdown hard break: ours keeps 2 sentences, ventilate collapses to one line.
    assert_ne!(our_groups("Roses are red,  \nViolets are blue.", &[]),
        repar_groups("Roses are red,  \nViolets are blue.", &[]),
        "L5 hard break: ours preserves the authored break (R2 exception); ventilate cannot");
}

// ── §6.5 second differential corpus: pragmatic_segmenter English "Golden Rules" ──
// Characterizes our intentionally-small detector against a full external segmenter.
// Equality set = rules R1–R4 reproduce; divergence ledger = rules we knowingly miss,
// each with a governing-decision reason. Empirically partitioned 2026-07-12 (34/18).
// GR = golden-rule number. Do NOT grow ABBREV_ALWAYS_MERGE to pass one; record reality.

fn golden(expected: &[&str]) -> Vec<Vec<String>> {
    expected.iter()
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .collect()
}

#[test]
fn golden_rules_equality() {
    // (GR, input, expected split) — our_groups(input, &[]) == golden(expected).
    let cases: &[(&str, &[&str])] = &[
        /* 1  */ ("Hello World. My name is Jonas.", &["Hello World.", "My name is Jonas."]),
        /* 2  */ ("What is your name? My name is Jonas.", &["What is your name?", "My name is Jonas."]),
        /* 3  */ ("There it is! I found it.", &["There it is!", "I found it."]),
        /* 4  */ ("My name is Jonas E. Smith.", &["My name is Jonas E. Smith."]),
        /* 6  */ ("Were Jane and co. at the party?", &["Were Jane and co. at the party?"]),
        /* 7  */ ("They closed the deal with Pitt, Briggs & Co. at noon.", &["They closed the deal with Pitt, Briggs & Co. at noon."]),
        /* 8  */ ("Let's ask Jane and co. They should know.", &["Let's ask Jane and co.", "They should know."]),
        /* 9  */ ("They closed the deal with Pitt, Briggs & Co. It closed yesterday.", &["They closed the deal with Pitt, Briggs & Co.", "It closed yesterday."]),
        /* 10 */ ("I can see Mt. Fuji from here.", &["I can see Mt. Fuji from here."]),
        /* 11 */ ("St. Michael's Church is on 5th st. near the light.", &["St. Michael's Church is on 5th st. near the light."]),
        /* 12 */ ("That is JFK Jr.'s book.", &["That is JFK Jr.'s book."]),
        /* 13 */ ("I visited the U.S.A. last year.", &["I visited the U.S.A. last year."]),
        /* 14 */ ("I live in the E.U. How about you?", &["I live in the E.U.", "How about you?"]),
        /* 15 */ ("I live in the U.S. How about you?", &["I live in the U.S.", "How about you?"]),
        /* 17 */ ("I have lived in the U.S. for 20 years.", &["I have lived in the U.S. for 20 years."]),
        /* 19 */ ("She has $100.00 in her bag.", &["She has $100.00 in her bag."]),
        /* 20 */ ("She has $100.00. It is in her bag.", &["She has $100.00.", "It is in her bag."]),
        /* 21 */ ("He teaches science (He previously worked for 5 years as an engineer.) at the local University.", &["He teaches science (He previously worked for 5 years as an engineer.) at the local University."]),
        /* 22 */ ("Her email is Jane.Doe@example.com. I sent her an email.", &["Her email is Jane.Doe@example.com.", "I sent her an email."]),
        /* 23 */ ("The site is: https://www.example.50.com/new-site/awesome_content.html. Please check it out.", &["The site is: https://www.example.50.com/new-site/awesome_content.html.", "Please check it out."]),
        /* 24 */ ("She turned to him, 'This is great.' she said.", &["She turned to him, 'This is great.' she said."]),
        /* 25 */ ("She turned to him, \"This is great.\" she said.", &["She turned to him, \"This is great.\" she said."]),
        /* 26 */ ("She turned to him, \"This is great.\" She held the book out to show him.", &["She turned to him, \"This is great.\"", "She held the book out to show him."]),
        /* 27 */ ("Hello!! Long time no see.", &["Hello!!", "Long time no see."]),
        /* 28 */ ("Hello?? Who is there?", &["Hello??", "Who is there?"]),
        /* 29 */ ("Hello!? Is that you?", &["Hello!?", "Is that you?"]),
        /* 30 */ ("Hello?! Is that you?", &["Hello?!", "Is that you?"]),
        /* 34 */ ("1) The first item. 2) The second item.", &["1) The first item.", "2) The second item."]),
        /* 40 */ ("This is a sentence\ncut off in the middle because pdf.", &["This is a sentence\ncut off in the middle because pdf."]),
        /* 41 */ ("It was a cold \nnight in the city.", &["It was a cold \nnight in the city."]),
        /* 44 */ ("She works at Yahoo! in the accounting department.", &["She works at Yahoo! in the accounting department."]),
        /* 46 */ ("Thoreau argues that by simplifying one's life, \"the laws of the universe will appear less complex. . . .\"", &["Thoreau argues that by simplifying one's life, \"the laws of the universe will appear less complex. . . .\""]),
        /* 48 */ ("If words are left off at the end of a sentence, and that is all that is omitted, indicate the omission with ellipsis marks (preceded and followed by a space) and then indicate the end of the sentence with a period . . . . Next sentence.", &["If words are left off at the end of a sentence, and that is all that is omitted, indicate the omission with ellipsis marks (preceded and followed by a space) and then indicate the end of the sentence with a period . . . .", "Next sentence."]),
        /* 49 */ ("I never meant that.... She left the store.", &["I never meant that....", "She left the store."]),
    ];
    for (input, expected) in cases {
        assert_eq!(our_groups(input, &[]), golden(expected), "golden equality mismatch: {input:?}");
    }
}

#[test]
fn golden_rules_accepted_divergences() {
    // (input, golden-expected split, reason). assert_ne — a vanishing divergence fails.
    let cases: &[(&str, &[&str], &str)] = &[
        // §11 out-of-scope — numbered / bulleted / alpha lists (no list-marker model)
        ("1.) The first item 2.) The second item", &["1.) The first item", "2.) The second item"], "GR31 §11 list markers"),
        ("1.) The first item. 2.) The second item.", &["1.) The first item.", "2.) The second item."], "GR32 §11 list markers"),
        ("1) The first item 2) The second item", &["1) The first item", "2) The second item"], "GR33 §11 list markers (UAX under-splits)"),
        ("1. The first item 2. The second item", &["1. The first item", "2. The second item"], "GR35 §11 list markers"),
        ("1. The first item. 2. The second item.", &["1. The first item.", "2. The second item."], "GR36 §11 list markers"),
        ("• 9. The first item • 10. The second item", &["• 9. The first item", "• 10. The second item"], "GR37 §11 bullet list"),
        ("⁃9. The first item ⁃10. The second item", &["⁃9. The first item", "⁃10. The second item"], "GR38 §11 hyphen-bullet list"),
        ("a. The first item b. The second item c. The third list item", &["a. The first item", "b. The second item", "c. The third list item"], "GR39 §11 alpha list"),
        // §11 out-of-scope — other edge forms
        ("Please turn to p. 55.", &["Please turn to p. 55."], "GR5 §11 single-lowercase citation abbr not in the frozen §5 list"),
        ("At 5 a.m. Mr. Smith went to the bank. He left the bank at 6 P.M. Mr. Smith then went to the store.", &["At 5 a.m. Mr. Smith went to the bank.", "He left the bank at 6 P.M.", "Mr. Smith then went to the store."], "GR18 §11 a.m./p.m. time abbreviation (interior-dot + capital follow)"),
        ("You can find it at N°. 1026.253.553. That is where the treasure is.", &["You can find it at N°. 1026.253.553.", "That is where the treasure is."], "GR43 §11 geo-coordinates"),
        ("\"Bohr [...] used the analogy of parallel stairways [...]\" (Smith 55).", &["\"Bohr [...] used the analogy of parallel stairways [...]\" (Smith 55)."], "GR47 §11 parenthetical citation after quotation (markup-blind, §10 R3 note)"),
        ("I wasn't really ... well, what I mean...see . . . what I'm saying, the thing is . . . I didn't mean it.", &["I wasn't really ... well, what I mean...see . . . what I'm saying, the thing is . . . I didn't mean it."], "GR50 §11 spaced-ellipsis edge form"),
        ("One further habit which was somewhat weakened . . . was that of combining words into self-interpreting compounds. . . . The practice was not abandoned. . . .", &["One further habit which was somewhat weakened . . . was that of combining words into self-interpreting compounds.", ". . . The practice was not abandoned. . . ."], "GR51 §11 4-dot ellipsis grouping edge form"),
        ("Hello world.Today is Tuesday.Mr. Smith went to the store and bought 1,000.That is a lot.", &["Hello world.", "Today is Tuesday.", "Mr. Smith went to the store and bought 1,000.", "That is a lot."], "GR52 §11 no whitespace between sentences (UAX SB needs whitespace)"),
        // §10 residue — grammar ambiguity, S7 POS resolves
        ("I work for the U.S. Government in Virginia.", &["I work for the U.S. Government in Virginia."], "GR16 §10 abbrev + capitalized proper noun (the St. Louis ambiguity)"),
        ("We make a good team, you and I. Did you see Albert I. Jones yesterday?", &["We make a good team, you and I.", "Did you see Albert I. Jones yesterday?"], "GR45 §10 'I.' pronoun-vs-initial (single-capital rule merges; §4.4 cost)"),
        // R2-by-design — the DOMINANT reflow hard-wrap merge deliberately opposes the golden rule
        ("features\ncontact manager\nevents, activities\n", &["features", "contact manager", "events, activities"], "GR42 R2 by design: newline-separated unterminated fragments merge (§1/§4 reflow fix)"),
    ];
    for (input, expected, reason) in cases {
        assert_ne!(our_groups(input, &[]), golden(expected), "golden divergence vanished — {reason}: {input:?}");
    }
}
```

> **Implementer note:** confirm `run_transform` and `TransformKind` are `pub` and reachable as
> `wordcartel::transform::{run_transform, TransformKind}` (they are: `pub mod transform;`
> `lib.rs:32`, `pub fn run_transform` `transform.rs:326`, `pub enum TransformKind` `transform.rs:8`).
> If a specific case's live repar output disagrees with the equality/ledger split, that is a
> **finding to surface** (a divergence the spec did not anticipate), not a value to silently flip —
> raise it. The 15 repar-equality cases + 6 repar-ledger asserts, and all 34 golden-equality + 18
> golden-divergence asserts, were each run against live repar / our detector with the shell fixup
> stack while authoring this plan; they pass. The golden inputs are the canonical English Golden
> Rules from `github.com/diasks2/pragmatic_segmenter` (transcribe them character-for-character; the
> escaped quotes/backslashes in the table are exact).

### 5.2 Verify green

`cargo test -p wordcartel --test sentence_differential` (all four fns:
`equality_corpus_agrees_with_ventilate`, `divergence_ledger`, `golden_rules_equality`,
`golden_rules_accepted_divergences`); `cargo clippy -p wordcartel --all-targets`;
`cargo test --workspace`.

### 5.3 Commit

`test(shell): S5 sentence differential suite — repar ventilate + golden-rules corpora`

---

## Task 6 — Shell: focus-mode §9 behavior-change test

**Crate/file:** `wordcartel/src/e2e.rs`.
**Command-surface conformance:** N/A — test-only.
**Depends on:** Task 1 (content-only `sentence_bounds` feeds `gather_row_ctx`).

### 6.1 The test — assert the §9 change: Focus=Sentence now focuses a real (multi-row) sentence

Add a `dim_cols` helper on `Harness` (mirror `underlined_cols`, `e2e.rs:258-263`, swapping
`UNDERLINED` → `DIM`) and a test. The document is a single paragraph whose FIRST sentence hard-wraps
across two visual rows and whose SECOND sentence is on a later row; under Focus=Sentence with the
caret in the first sentence, BOTH rows of sentence 1 stay bright and the sentence-2 row dims.

```rust
    /// Column indices on visual row `y` that carry the DIM modifier (focus dimming).
    fn dim_cols(&self, y: u16) -> Vec<u16> {
        use ratatui::style::Modifier;
        let buf = self.term.backend().buffer();
        let w = buf.area().width;
        (0..w).filter(|&x| buf[(x, y)].style().add_modifier.contains(Modifier::DIM)).collect()
    }
```

```rust
#[test]
fn e2e_focus_sentence_spans_wrapped_rows_not_just_a_line() {
    // A hard-wrapped paragraph: sentence 1 wraps two rows; sentence 2 follows.
    // Pre-S5, Focus=Sentence dimmed row 2 of sentence 1 (it focused only a LINE).
    let text = "The committee met on Tuesday and the\nchair insisted on a vote. Then we left.\n";
    let mut h = Harness::new(text, None, (40, 10));
    {
        let mut ed = h.editor.borrow_mut();
        ed.view_opts.focus = true;
        ed.view_opts.focus_granularity = crate::config::FocusGranularity::Sentence;
        // caret in sentence 1 (byte 5, "committee")
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(5);
        crate::derive::rebuild(&mut ed);
    }
    h.render();
    // Rows 0 and 1 are sentence 1 (hard-wrapped) → NOT dimmed.
    assert!(h.dim_cols(0).is_empty(), "row 0 (sentence 1) must not be dimmed");
    assert!(h.dim_cols(1).is_empty(), "row 1 (sentence 1 cont.) must not be dimmed — the S5 fix");
    // The row carrying "Then we left." (sentence 2) IS dimmed.
    let sentence2_row = (0..10u16).find(|&y| h.row(y).contains("Then we left"))
        .expect("sentence 2 must be visible");
    assert!(!h.dim_cols(sentence2_row).is_empty(),
        "sentence 2 row must be dimmed while the caret is in sentence 1");
}
```

> **Implementer notes:** (1) confirm the `Harness::new` signature and the `render()` / `row(y)`
> helpers (`e2e.rs` ~30, 197, 244) — they exist; borrow the editor via `h.editor.borrow_mut()` (the
> field is in-module accessible). (2) The exact row where the wrap lands depends on width 40 and the
> render's soft-wrap; if row indices differ, locate sentence-1 rows by `h.row(y).contains("committee")`
> / `contains("chair")` and sentence-2 by `contains("Then we left")` rather than hard-coding 0/1 —
> the *behavior* pinned is "both sentence-1 rows bright, sentence-2 row dim." (3) If focus dimming
> requires a color depth / theme like the existing render focus tests (`render.rs:2646` sets
> `no_color()` + `Depth::None`), replicate that setup in the harness editor before `render()`.

### 6.2 Verify green

`cargo test -p wordcartel e2e_focus_sentence`; `cargo clippy -p wordcartel --all-targets`;
`cargo test --workspace`.

### 6.3 Commit

`test(shell): e2e Focus=Sentence spans a wrapped sentence, not a line (S5 §9)`

---

## Test-plan coverage map (spec §12 → tasks)

| Spec test | Where it lands |
|---|---|
| T-1 flipped pin `(0,9)→(0,8)` | Task 1 `sentence_bounds_basic` |
| T-2 empty/whitespace | Task 1 `sentence_spans_empty_and_whitespace`, `sentence_bounds_attach_rule`; Task 2 `sentence_motion_kernels_empty` |
| T-3 R1 / abbreviation classes (+ dropped `no`, case-insensitive) | Task 1 `r1_abbreviations_merge`, `r1_non_abbreviations_and_dropped_no_break` |
| T-4 R2 + GLOBAL hard-break veto (gates R1/R2/R3; incl. locking fixtures) | Task 1 `r2_hard_wrap_merges`, `hard_break_is_a_global_merge_veto` |
| T-5 R3 | Task 1 `r3_lowercase_after_quote_merges` |
| T-6 R4 | Task 1 `r4_shifts_past_closing_markup` |
| T-7 UAX preservation | Task 1 `uax_wins_are_preserved` |
| T-8 attach rule + multibyte | Task 1 `sentence_bounds_attach_rule`, `multibyte_offsets_are_safe` |
| T-9 expand-ladder regression | Task 3 `expand_ladder_sentence_rung_survives_single_sentence_paragraph` |
| T-10 motion start/end + cross-block BOTH directions + extend | Task 3 `sentence_motion_start_and_end`, `sentence_motion_crosses_blocks_both_directions`, `sentence_motion_extends_selection` |
| T-11 registry dispatch (handlers RUN) + palette gate | Task 4 `sentence_motion_commands_dispatch_and_take_effect` + existing `palette.rs:255` |
| T-12 chord resolution CUA (4 bound) + WordStar (all 4 unbound) | Task 4 `sentence_motions_bound_in_cua_unbound_in_wordstar` + existing hint gates |
| T-13 differential suite (repar + golden rules) | Task 5 `equality_corpus_agrees_with_ventilate`, `divergence_ledger`, `golden_rules_equality`, `golden_rules_accepted_divergences` |
| T-14 focus-mode behavior change | Task 6 `e2e_focus_sentence_spans_wrapped_rows_not_just_a_line` |

---

## Pipeline status

**Plan: AUTHORED (2026-07-12) — entering the Codex plan gate (loop to clean).**
**Not yet done:** Codex plan gate → subagent-driven TDD execution → Fable whole-branch + Codex
pre-merge gates → `--no-ff` merge.

---

## FLAGS — decisions forced while turning the spec into concrete code (highest gate risk)

**FLAG 1 — RESOLVED 2026-07-12 (human ratified Option A: GLOBAL merge veto).** The hard-break check
gates R1, R2, AND R3 — nothing merges across a semantic hard break (`!semantic_hard_break(..) &&
(r1 || r2 || r3)`, the `merges()` helper). Spec §4.5 (+ §4.1/§4.2 pseudocode) and §4.9/T-4 were
amended to match; locking fixtures `"Dr.  \nSmith went home."` and `"See fig.\\\nTwo shows it."`
(→ 2 sentences each) and the lowercase-continuation `"…red\\\n…two…"` case are pinned in Task 1's
`hard_break_is_a_global_merge_veto` test. Consequence (documented in the spec History): an
abbreviation immediately before an authored hard break splits rather than merges — safe
(over-segments, never swallows authored content). **No open question remains.**

**FLAG 2 (plan-level decision, low risk). Iterator shape = one-slot `pending` pushback, no carry
struct.** The spec §4.2 floated "a small carry for an R4-consumed segment remainder." I collapsed
that into a single `pending: Option<(usize,usize)>` slot that serves three roles (un-consumed next
unit, R4 remainder, and whitespace-skip lookahead), which is simpler than a separate carry field and
stays allocation-free. No design impact; noting it because the spec's struct sketch differs.

**FLAG 3 (plan-level, low risk). `content_end` returns the ABSOLUTE byte end** (`text[..raw_end]
.trim_end_matches(ws).len()`), exploiting that the slice starts at 0. This is a micro-idiom, not a
design point; flagged only so a reviewer doesn't read it as a bug.

**Non-flag confirmations:** no shell test pins the old trailing-space span (the flip is
workspace-safe); the `Move` `match dir` is the *only* exhaustive match over `commands::Dir` (one
forced-arm site); `Alt+a`/`Alt+e`/`Alt+shift-a`/`Alt+shift-e` are all free in CUA; the equality
corpus and all six ledger `assert_ne!`s were each run against live repar ventilate with the shell
fixup stack.
