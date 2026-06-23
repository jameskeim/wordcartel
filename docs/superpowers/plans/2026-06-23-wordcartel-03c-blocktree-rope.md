# Wordcartel block_tree Rope Integration — Implementation Plan (Effort 3c)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `block_tree` reparse from a **text source that can be a `ropey::Rope`**, materializing only the **edited region** per incremental update, so the terminal shell's per-keystroke derive is O(visible)+O(edited) — not O(document) — honoring §3.9. The pure-string API stays for the oracle and existing callers.

**Architecture:** Introduce a `TextSource` trait (random byte access + region slice + `\n`-line boundaries), implement it for `&str` (today's behavior, zero-cost) and `&ropey::Rope` (region slices via `byte_slice(..).to_string()`, O(region); line boundaries via the rope's line index). Thread `TextSource` through `full_parse`, `incremental_update`, and every text-indexing helper. The existing **incremental == full oracle is the merge gate**, extended to run the entire corpus through BOTH a `&str` source and a `&Rope` source and to assert the two paths agree.

**Tech Stack:** Rust 2021; existing `pulldown-cmark 0.13`, `ropey =1.6.1` (already a core dep), `proptest`. Pure/headless. This is a behavior-preserving refactor of the project's highest-bug-surface module — correctness rides entirely on the oracle.

## Global Constraints

- Crate `wordcartel-core`; `#![forbid(unsafe_code)]`; pure/headless.
- Canonical position = **byte offset** (`usize`); spans absolute.
- **Behavior-preserving:** the `&str` path must produce byte-identical `BlockTree`s to today (all current block_tree unit tests + the oracle stay green unchanged). The `&Rope` path must produce identical trees to the `&str` path for the same content.
- **Line-boundary semantics must match exactly.** Today's `line_start`/`line_end` split on `\n` only (scanning bytes). The `Rope` impl MUST reproduce the same `\n`-only boundaries — do NOT silently adopt ropey's default multi-line-break set (LF/CR/CRLF/VT/FF/NEL/LS/PS), which would diverge from the `&str` path and break the oracle. (If you use ropey line APIs, verify they were built with `\n`-only semantics for our text; otherwise scan for `\n`.) The oracle's multibyte/multiline corpus is the gate that catches a mismatch.
- **Performance intent:** a *local* edit (no widen-to-end) must materialize only O(region) bytes of the new text, and read only O(region + edited-line-length) bytes of old/new via the source — never O(document). The full-fallback paths (HTML, widen-to-end) MAY slice large regions; that's their existing nature and is acceptable (rare).
- The oracle MUST NOT be weakened. If a counterexample appears, the refactor diverged from the validated algorithm — fix the refactor.
- TDD; pristine output; commit `proptest-regressions/` seeds.

---

## Reuse Posture

This is a refactor of our own validated module. We change HOW text is accessed (trait-dispatched) while preserving the validated invalidation algorithm exactly. `ropey` is already the buffer (§3.10) and provides O(1) snapshots (§10.3) — the shell will pass a pre-edit rope snapshot and the post-edit rope, so an incremental update slices only the edited region from each. No new dependency.

---

## File Structure

- `wordcartel-core/src/block_tree.rs` — add `TextSource` trait + `&str`/`&Rope` impls; refactor `full_parse`, `parse_region`, `incremental_update`/`_instrumented`, and the text-indexing helpers (`line_start`, `line_end`, `needs_widen_to_end`, `html_in_play`, `edit_touches_fence_line`, `contains_ref_def`, `is_ref_def_line`, `blank_delimited_group_start`, the gap-slice in the absorptive branch) to take `&S: TextSource`. Keep `&str`-typed public wrappers; add `&Rope`-typed public entry points.
- `wordcartel-core/tests/block_tree_oracle.rs` — extend the proptest oracle + a few hazard regressions to exercise the `&Rope` source path and assert `str-path == rope-path == full`.

---

### Task 1: `TextSource` trait + `&str` and `&Rope` impls

**Files:** `wordcartel-core/src/block_tree.rs`.

**Interfaces — Produces:**
```rust
use std::borrow::Cow;
/// Random-access view over the document text for block parsing.
/// Byte offsets are into the whole document. `slice` returns a CONTIGUOUS &str
/// (borrowed for &str sources, owned/materialized for ropes — O(slice len)).
pub trait TextSource {
    fn len(&self) -> usize;
    fn slice(&self, range: std::ops::Range<usize>) -> Cow<'_, str>;
    /// Byte offset of the start of the line containing `pos` (just after the
    /// previous `\n`, or 0). `\n`-only semantics. `pos` is clamped to `len()`.
    fn line_start(&self, pos: usize) -> usize;
    /// Byte offset of the end of the line containing `pos` (the next `\n`, or `len()`).
    fn line_end(&self, pos: usize) -> usize;
}
impl TextSource for &str { /* slice = Cow::Borrowed(&self[range]); line_start/end scan bytes for '\n' (PORT today's free fns verbatim) */ }
impl TextSource for &ropey::Rope { /* slice = Cow::Owned(self.byte_slice(range).to_string()); line_start/end find the nearest '\n' boundaries */ }
```
The `&str` `line_start`/`line_end` bodies are EXACTLY today's free `line_start`/`line_end` functions (UTF-8-safe byte scan — preserve the 3a fix). The `&Rope` impl must yield identical results (same `\n`-only boundaries) — implement by scanning for `\n` via the rope (e.g. `rope.bytes_at(pos)` forward / a reverse scan), or via the rope line index ONLY if verified `\n`-equivalent for the test corpus.

- [ ] **Step 1: Write failing tests** — for a set of strings (ASCII, multibyte `中`/`🙂`, multiple lines, no trailing newline, empty), build both `s: &str` and `Rope::from_str(s)`, and assert: `src.len()` equal; `src.slice(a..b)` equal for several ranges incl. line-aligned and multibyte-content ranges; `src.line_start(p)` and `src.line_end(p)` equal across many `p` (including line starts, mid-line, on a `\n`, at `len()`).
```rust
#[test]
fn textsource_str_and_rope_agree() {
    for s in ["", "a", "a\n", "\n", "ab\ncd\n", "# 中\n\n🙂 x\nyy"] {
        let r = ropey::Rope::from_str(s);
        let a: &dyn TextSource = &s; // note: impls are for &str / &Rope
        // compare len, slice over a few ranges, line_start/line_end over all p in 0..=s.len()
        // (see helper in test) — assert (&s) path == (&r) path byte-for-byte.
    }
}
```
- [ ] **Step 2:** Run `cargo test -p wordcartel-core textsource` → FAIL.
- [ ] **Step 3:** Implement the trait + both impls. Port the `&str` `line_start`/`line_end` from the existing free functions verbatim.
- [ ] **Step 4:** Run → PASS (incl. multibyte).
- [ ] **Step 5:** Commit: `feat(block_tree): TextSource trait (&str + &Rope), \n-line semantics`

---

### Task 2: Refactor `full_parse` / `parse_region` over `TextSource`

**Files:** `wordcartel-core/src/block_tree.rs`.

**Interfaces — Produces:** `pub fn full_parse_src<S: TextSource>(src: &S) -> BlockTree` and keep `pub fn full_parse(text: &str) -> BlockTree { full_parse_src(&text) }` as a wrapper. `parse_region` becomes `fn parse_region<S: TextSource>(src: &S, region: Range<usize>, base: usize) -> BlockTree` (it now slices `src.slice(region)` internally rather than receiving a `&str` already-slice + base). Internally it still calls `crate::md_parse`-free pulldown parsing on the materialized `Cow<str>` region.

- [ ] **Step 1: Write failing test:** `full_parse_src(&Rope::from_str(s)) == full_parse(s)` for a handful of representative docs (heading+para, fenced code w/ blanks, nested list, blockquote, table, ref-def, multibyte).
- [ ] **Step 2:** Run → FAIL (no `full_parse_src`).
- [ ] **Step 3:** Refactor. `full_parse` slices the whole `src` once (O(doc), as today, on load). Keep span math identical.
- [ ] **Step 4:** Run → PASS; confirm ALL existing block_tree unit tests + the oracle still pass unchanged (the `&str` path is behavior-identical).
- [ ] **Step 5:** Commit: `refactor(block_tree): full_parse over TextSource (str path unchanged)`

---

### Task 3: Refactor incremental helpers over `TextSource`

**Files:** `wordcartel-core/src/block_tree.rs`.

**Interfaces — Produces:** the text-indexing private helpers take `&S: TextSource` and replace every `&text[a..b]` with `src.slice(a..b)` and every `line_start(text, p)`/`line_end(text, p)` call with `src.line_start(p)`/`src.line_end(p)`: `needs_widen_to_end`, `html_in_play`, `edit_touches_fence_line`, `contains_ref_def`/`is_ref_def_line` (these take a line `&str` — feed them `&src.slice(line_range)`), `blank_delimited_group_start`, and the absorptive-branch gap slice (`&old_text[gap_lo..gap_hi]` → `old_src.slice(gap_lo..gap_hi)`). The free `line_start`/`line_end` functions are removed (their logic now lives in the `&str` `TextSource` impl from Task 1).

- [ ] **Step 1: Write failing tests** — none new yet; this task is covered by re-running the existing oracle after the refactor (Step 3). (If the crate won't compile mid-refactor, that's expected until all helpers are converted.)
- [ ] **Step 2:** (skip — compile-driven)
- [ ] **Step 3:** Convert each helper. Keep each function's logic byte-identical; only the text-access changes. Build clean.
- [ ] **Step 4:** Run the FULL block_tree oracle (`cargo test -p wordcartel-core --test block_tree_oracle`) and unit tests → all PASS unchanged (the `&str` path is still exercised via `incremental_update(&str)`, which Task 4 rewires; until then, keep `incremental_update` calling the helpers with `&old_text`/`&new_text` as `&str` sources so the suite stays green at each commit).
- [ ] **Step 5:** Commit: `refactor(block_tree): incremental helpers over TextSource`

---

### Task 4: Refactor `incremental_update` over `TextSource` + rope entry points

**Files:** `wordcartel-core/src/block_tree.rs`.

**Interfaces — Produces:**
```rust
pub fn incremental_update_src<S: TextSource>(old_tree: &BlockTree, old_src: &S, edit: &Edit, new_src: &S) -> BlockTree;
pub fn incremental_update_instrumented_src<S: TextSource>(old_tree: &BlockTree, old_src: &S, edit: &Edit, new_src: &S) -> UpdateOutcome;
// &str wrappers (unchanged signatures for existing callers/oracle):
pub fn incremental_update(old_tree: &BlockTree, old_text: &str, edit: &Edit, new_text: &str) -> BlockTree
    { incremental_update_src(old_tree, &old_text, edit, &new_text) }
// rope entry points the shell calls:
pub fn full_parse_rope(rope: &ropey::Rope) -> BlockTree { full_parse_src(&rope) }
pub fn incremental_update_rope(old_tree: &BlockTree, old_rope: &ropey::Rope, edit: &Edit, new_rope: &ropey::Rope) -> BlockTree
    { incremental_update_src(old_tree, &old_rope, edit, &new_rope) }
```
The body of `incremental_update_instrumented_src` is today's `incremental_update_instrumented` with every `old_text`/`new_text` index/slice/`.len()` routed through `old_src`/`new_src` (`.len()` → `src.len()`, `&new_text[region]` → `new_src.slice(region)`, etc.). The final `Block { kind: Document, span: 0..new_src.len(), .. }`.

- [ ] **Step 1: Write failing test:** `incremental_update_rope` equals `full_parse_rope(new)` on a representative local edit, and equals the `&str` path:
```rust
#[test]
fn rope_incremental_matches_full_and_str() {
    let old = "para one\n\n- a\n- b\n\n[r]: http://x\n";
    let (new, edit) = apply_edit(old, 9..9, "X");      // edit inside the list region
    let ot = full_parse(old);
    let str_tree = incremental_update(&ot, old, &edit, &new);
    let rope_tree = incremental_update_rope(&ot, &ropey::Rope::from_str(old), &edit, &ropey::Rope::from_str(&new));
    assert_eq!(str_tree, full_parse(&new));
    assert_eq!(rope_tree, str_tree);
}
```
- [ ] **Step 2:** Run → FAIL.
- [ ] **Step 3:** Refactor the body over the source params; wire wrappers + rope entry points.
- [ ] **Step 4:** Run → PASS; full block_tree oracle + unit suite green unchanged.
- [ ] **Step 5:** Commit: `feat(block_tree): incremental_update over TextSource + rope entry points`

---

### Task 5: Oracle extension — str-path == rope-path == full (the gate)

**Files:** `wordcartel-core/tests/block_tree_oracle.rs`.

**Interfaces — Produces:** every proptest oracle case (single-edit, multi-edit chain, multibyte, multibyte-chain) ALSO builds ropes from the generated old/new strings and asserts the rope path equals both the `&str` incremental path and `full_parse`. Add a helper:
```rust
fn assert_all_paths_agree(old: &str, edit: &Edit, new: &str) {
    let ot = full_parse(old);
    let full = full_parse(new);
    let str_inc = incremental_update(&ot, old, edit, new);
    let rope_inc = incremental_update_rope(&ot, &Rope::from_str(old), edit, &Rope::from_str(new));
    prop_assert_eq!(&str_inc, &full);
    prop_assert_eq!(&rope_inc, &full);   // rope path correct
    prop_assert_eq!(&rope_inc, &str_inc); // and identical to str path
}
```
Route the existing oracle proptests (and ~3 named hazard regressions, incl. the CE1 table + CE2 list cases and a multibyte case) through `assert_all_paths_agree`.

- [ ] **Step 1:** Add the helper + route the proptests/regressions through it. Run `cargo test -p wordcartel-core --test block_tree_oracle` at the existing case count → must PASS.
- [ ] **Step 2:** Shake out: run at high counts and multiple seeds: `for i in 1 2 3 4 5 6; do PROPTEST_CASES=2500 cargo test -p wordcartel-core --test block_tree_oracle || break; done` → all green. If the rope path diverges, the `&Rope` `line_start`/`line_end` or `slice` doesn't match the `&str` semantics (likely the `\n`-only line-break issue) — fix the impl; do NOT weaken the oracle. Commit any new `proptest-regressions/` seeds.
- [ ] **Step 3:** Full suite green (`cargo test -p wordcartel-core`), no warnings. Commit: `test(block_tree): oracle covers str==rope==full across the corpus`

---

## Self-Review (completed during planning)

- **Spec coverage:** §3.9 hot-path-is-O(visible) — this effort is the enabler (block_tree reparses O(region) from the rope); §10.3 O(1) rope snapshots feed it (the shell passes pre/post-edit ropes in Plan 4a). The incremental==full oracle (§11.2) is preserved and strengthened to cover the rope path.
- **Reuse:** refactors our own oracle-validated module; no new deps (`ropey` already present). The algorithm is unchanged — only text access is trait-dispatched.
- **Risk:** highest-bug-surface module. Mitigation: every task keeps the `&str` path behavior-identical (existing tests green at each commit), and Task 5 proves `rope == str == full` across the full corpus at high case counts. The one real hazard — `\n`-line-break semantics in the rope impl — is called out in Global Constraints and gated by the multibyte/multiline oracle.
- **Placeholder scan:** concrete signatures, exact helper list to convert, real tests; the one "compile-driven" task (3) is explicitly a mechanical conversion verified by the unchanged oracle.
- **Type consistency:** `TextSource` (T1) consumed by `full_parse_src`/`parse_region` (T2), the helpers (T3), and `incremental_update_src` + rope entry points (T4); the oracle (T5) calls `incremental_update_rope`/`full_parse_rope`.

## Completion

When all tasks are `- [x]`, `cargo test -p wordcartel-core` is green (oracle covers str==rope==full at high counts), and there are no warnings: mark Effort 3c complete in the coverage ledger. Then **revise Plan 4a** per its "PENDING REVISIONS" block — Task 3 derive uses `incremental_update_rope` with an O(1) pre-edit rope snapshot threaded through `apply` (and apply the rest of the Codex red-team fixes) — and execute 4a.
