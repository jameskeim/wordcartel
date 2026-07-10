# H7 — Panic-safety & arithmetic-soundness audit — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make the incremental block-tree region math and the two flagged shell sites degrade SAFELY under a broken precondition (clamp, never wrap/panic), and lock cast-soundness into the pure kernel with a crate-local clippy gate — with zero behavior change under the proven invariants.

**Architecture:** Blast-radius calibration, not uniform fail-loud. The mutation path (`buffer.rs`/`change.rs` release `assert!`s) is untouched — a bad position there corrupts the document, so loud-and-correct is right forever. The incremental parse path only mis-sizes a reparse region (cosmetic, self-heals on the next reconcile, and is already wrapped in `panicx::catch`), so each offset site keeps its `debug_assert!` as the dev/fuzz guard and gains a release fall-through that clamps instead of wrapping. The shell cursor casts are cosmetic — silent saturating clamps, no assert.

**Tech Stack:** Rust workspace (wordcartel-core pure `#![forbid(unsafe_code)]` + wordcartel shell), ratatui 0.30 / crossterm (`TestBackend` for render tests). No new dependencies.

**Spec (source of truth):** `docs/superpowers/specs/2026-07-10-h7-panic-arithmetic-audit-design.md` (Codex READY r2, human-approved). Every code shape below was re-verified against the current branch `effort-h7-panic-arithmetic-audit`; line anchors are current-as-of-writing and WILL drift — locate by symbol name.

## Global Constraints

- Do NOT run `cargo fmt` (repo is hand-formatted, dense; no rustfmt.toml). Match neighbors by hand; do not reflow untouched code.
- House style: snake_case/PascalCase/SCREAMING_SNAKE; 4-space indent; ~100-col hand-wrapped; em-dash `—` in prose comments, never `--`; NO emoji in code; private fields + accessors.
- GATES (before merge): `cargo test` green across all suites (`wordcartel-core` lib + oracle, `wordcartel` lib); `cargo build` + `cargo test --no-run` warning-free for touched crates; **`cargo clippy --workspace --all-targets` clean is a merge GATE** (workspace denies `clippy::all` + `too_many_lines` threshold 100; after T2 the core crate additionally denies the three cast lints).
- `wordcartel/tests/module_budgets.rs` must stay green (`render.rs` ≤ 900 production lines) — do NOT edit the budgets file.
- Anchor on SYMBOL NAMES, not line numbers, when locating sites. For compile/usage/signature questions on code you are editing, trust `cargo` + `grep`, NOT an editor "unused"/"undefined" hint.
- NON-GOAL — do NOT convert the 57 guarded `.unwrap()`s or the 13 informative `.expect()`s to anything; they are verified correct. Do NOT touch the mutation-path release `assert!`s in `buffer.rs`/`change.rs`. Do NOT introduce a `BytePos` newtype.
- Every commit ends with these trailers, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG
  ```

## Command-surface contract

**N/A — H7 does not touch commands, user-settable options, the palette, the menu, or keybinding hints.** `build_multi_replace` (T3) is an internal changeset builder behind existing commands; its hardening changes no command semantics. The contract's invariant tests run unchanged in the merge gate.

## File Structure

| File | Change | Task |
|---|---|---|
| `wordcartel-core/src/block_tree.rs` | Modify | T1: add `shift_offset` helper; route 8 offset sites through it; band-clamp sites 3/4; guardrail tests. T2: item-local cast allows. |
| `wordcartel-core/src/lib.rs` | Modify | T2: crate-level `#![deny(...)]` cast gate beside `#![forbid(unsafe_code)]`. |
| `wordcartel-core/src/theme.rs` | Modify | T2: item-local cast allows on 4 color-math fns. |
| `wordcartel/src/commands.rs` | Modify | T3: `build_multi_replace` well-formedness guard + regression tests. |
| `wordcartel/src/render.rs` | Modify | T4: `place_cursor` compute-in-usize-then-guard; cursor-clamp test. |
| `wordcartel/src/nav.rs` | Modify | T4: `screen_pos` saturating vcol narrow. |

Tests are co-located `#[cfg(test)] mod tests` (`use super::*`) in each touched module.

Task order **T1 → T2 → T3 → T4**: T2 runs after T1 so every core cast is in its final `shift_offset` form before the gate turns on; T3 and T4 are independent of each other and of T1/T2.

---

### Task 1: `shift_offset` helper + core parse-path offset sites (block_tree.rs)

Add one file-local helper and route all eight `(pos as isize + delta) as usize`-class sites through it, with a release band-clamp at the two unclamped slice-feeding sites and the persisting-into-tree site. Under the region invariants every sum is provably `>= 0` (spec §4.1 lemma L1); the `debug_assert!` is the dev/fuzz guard, the clamp is the release fall-through. **Do NOT add any `#[allow(clippy::cast_*)]` in this task** — the cast gate is off until T2, so the tree stays clippy-clean without them.

**Files:**
- Modify: `wordcartel-core/src/block_tree.rs` — new fn `shift_offset` (place it just above `fn shift_range`, near the end of the production code); edits at `Edit::delta` (leave as-is this task), `incremental_update_instrumented_src_owned` (`base_new_end`, the two `region_new_end`), `needs_widen_to_end`, `html_in_play`, `region_has_bare_cr`, `shift_range`.
- Test: `wordcartel-core/src/block_tree.rs` (existing `#[cfg(test)] mod tests` at the file bottom, `use super::*;`).

**Interfaces:**
- New: `fn shift_offset(pos: usize, delta: isize) -> usize` (module-private; visible to `mod tests` via `use super::*`).
- Existing test vocabulary reused: `apply_edit(&str, Range<usize>, &str) -> (String, Edit)`, `full_parse(&str) -> BlockTree`, `incremental_update_instrumented(&BlockTree, &str, &Edit, &str) -> UpdateOutcome`, `assert_valid_tree(&BlockTree, usize)`, and `UpdateOutcome { tree, reason, reparsed_bytes }`.

- [ ] **Step 1: Write the failing tests.** Append to `mod tests` in `wordcartel-core/src/block_tree.rs` (after the last existing test, inside the module):

```rust
    #[test]
    fn shift_offset_at_zero_boundary_clamps_not_wraps() {
        // The exact lower boundary pos+delta==0 must yield 0 (not wrap to a huge usize);
        // positive sums are exact. Underflow (pos+delta<0) is deliberately NOT tested here —
        // the debug_assert fires under `cargo test`, which is the dev guard working.
        assert_eq!(shift_offset(5, -5), 0);
        assert_eq!(shift_offset(0, 0), 0);
        assert_eq!(shift_offset(10, 3), 13);
        assert_eq!(shift_offset(10, -4), 6);
    }

    #[test]
    fn h7_delete_all_region_stays_in_bounds() {
        // Most-negative delta: 0..len -> "". region_new_end must clamp to 0, not wrap.
        let doc = "# H\n\npara\n\n- a\n- b\n";
        let (new_text, edit) = apply_edit(doc, 0..doc.len(), "");
        assert_eq!(new_text, "");
        let outcome = incremental_update_instrumented(&full_parse(doc), doc, &edit, &new_text);
        assert_valid_tree(&outcome.tree, new_text.len());
        assert_eq!(outcome.tree, full_parse(&new_text));
    }

    #[test]
    fn h7_insert_into_empty_is_in_bounds() {
        let doc = "";
        let (new_text, edit) = apply_edit(doc, 0..0, "# H\n\npara\n");
        let outcome = incremental_update_instrumented(&full_parse(doc), doc, &edit, &new_text);
        assert_valid_tree(&outcome.tree, new_text.len());
        assert_eq!(outcome.tree, full_parse(&new_text));
    }

    #[test]
    fn h7_replace_at_eof_is_in_bounds() {
        let doc = "para one\n\npara two\n";
        let start = "para one\n\n".len();
        let (new_text, edit) = apply_edit(doc, start..doc.len(), "changed tail\n");
        let outcome = incremental_update_instrumented(&full_parse(doc), doc, &edit, &new_text);
        assert_valid_tree(&outcome.tree, new_text.len());
        assert_eq!(outcome.tree, full_parse(&new_text));
    }

    #[test]
    fn h7_whole_doc_replace_is_in_bounds() {
        let doc = "old\n";
        let (new_text, edit) = apply_edit(doc, 0..doc.len(), "brand new doc\n\nsecond\n");
        let outcome = incremental_update_instrumented(&full_parse(doc), doc, &edit, &new_text);
        assert_valid_tree(&outcome.tree, new_text.len());
        assert_eq!(outcome.tree, full_parse(&new_text));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel-core -- shift_offset_at_zero_boundary_clamps_not_wraps h7_delete_all_region_stays_in_bounds h7_insert_into_empty_is_in_bounds h7_replace_at_eof_is_in_bounds h7_whole_doc_replace_is_in_bounds`
Expected: COMPILE ERROR — `cannot find function 'shift_offset' in this scope` (a compile failure is the correct red state when the function under test does not exist yet).

- [ ] **Step 3: Implement `shift_offset` + route the sites.**

(a) Add the helper immediately above `fn shift_range` (locate `fn shift_range` by name):

```rust
/// Shift an old-text byte offset by an edit `delta` into new-text coordinates.
/// The incremental region invariants make the sum non-negative for every call site
/// (spec §4.1: each shifted position `pos >= edit_hi`, so `pos + delta >= edit_lo +
/// new_len >= 0` by lemma L1). The `debug_assert!` is the dev/fuzz guard — the F2
/// oracle patrols the same regions — and the release fall-through clamps at 0: a
/// self-healing over-wide reparse beats a wrap to a garbage huge usize (the rope is
/// never mutated on this path; a bad region only mis-colours for ~150 ms).
fn shift_offset(pos: usize, delta: isize) -> usize {
    let shifted = pos as isize + delta;
    debug_assert!(
        shifted >= 0,
        "shift_offset underflow: pos {} + delta {} < 0 — incremental region invariant broken",
        pos, delta
    );
    shifted.max(0) as usize
}
```

(b) `shift_range` (the site-8 twin) — route both endpoints through it:

```rust
fn shift_range(r: &Range<usize>, delta: isize) -> Range<usize> {
    shift_offset(r.start, delta)..shift_offset(r.end, delta)
}
```

(c) In `incremental_update_instrumented_src_owned` — site 2, `base_new_end` (locate the line `let base_new_end = (base_end as isize + delta) as usize;`):

```rust
        let base_new_end = shift_offset(base_end, delta);
```

(d) Same fn — site 3, the first `region_new_end` (locate `let mut region_new_end = (region_old_end as isize + delta) as usize;`, which sits just after `let region_new_start = region_old_start;`). Add the release band-clamp so a violated precondition degrades to an over-wide-but-in-bounds slice instead of a panic:

```rust
    let region_new_start = region_old_start;
    // Band-clamp (H7): shift_offset guards the lower bound at 0; .min(len) is the
    // load-bearing slice-safety clamp; .max(region_new_start) keeps start<=end so the
    // slice below can never invert. Dead code under the proven invariant (§4.2 site 3).
    let mut region_new_end =
        shift_offset(region_old_end, delta).min(new_src.len()).max(region_new_start);
```

(e) Same fn — site 4, the second `region_new_end` inside the created-container re-widen branch (locate `region_new_end = (region_old_end as isize + delta) as usize;`, immediately after `region_old_end = old_src.len();`):

```rust
                region_new_end =
                    shift_offset(region_old_end, delta).min(new_src.len()).max(region_new_start);
```

(f) `needs_widen_to_end` — site 5 (locate `let new_region_end = ((region_old_end as isize + edit.delta()) as usize).min(new_src.len());`):

```rust
    let new_region_end = shift_offset(region_old_end, edit.delta()).min(new_src.len());
```

(g) `html_in_play` — site 6, identical shape and identical replacement:

```rust
    let new_region_end = shift_offset(region_old_end, edit.delta()).min(new_src.len());
```

(h) `region_has_bare_cr` — site 7, identical shape and identical replacement:

```rust
    let new_region_end = shift_offset(region_old_end, edit.delta()).min(new_src.len());
```

(i) `Edit::delta` (site 1) — LEAVE UNCHANGED this task. It is the deliberate signed delta (`self.new_len as isize - self.range.len() as isize`); its `#[allow]` lands in T2.

The four existing `debug_assert!`s in `incremental_update_instrumented_src_owned` (`region_old_end <= old_src.len()`, the two `region_old_start <= edit_lo`, `region_old_end >= edit_hi`) and the splice-order `debug_assert!(splice_lo <= splice_hi, …)` all STAY — do not remove or weaken them.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p wordcartel-core -- shift_offset_at_zero_boundary_clamps_not_wraps h7_delete_all_region_stays_in_bounds h7_insert_into_empty_is_in_bounds h7_replace_at_eof_is_in_bounds h7_whole_doc_replace_is_in_bounds`
Expected: PASS (5 tests). Then `cargo test -p wordcartel-core` — ALL core tests + the F2 oracle PASS (no regressions; the incremental≡full oracle still passes because behavior is unchanged under the invariants). Then `cargo clippy --workspace --all-targets` — clean (no new casts introduced; the gate is still off).

- [ ] **Step 5: Commit**

```bash
cd /home/jkeim/projects/groundwords
git add wordcartel-core/src/block_tree.rs
git commit -m "fix(core): H7 — clamp incremental region offsets instead of wrapping (shift_offset)

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG"
```

---

### Task 2: Core-crate cast-soundness gate + item-local allows

Turn on the three pedantic cast lints crate-wide in `wordcartel-core` ONLY, then add a reason-carrying item-local `#[allow(...)]` at each benign widening the gate now flags. The gate attribute AND every allow land in ONE commit so `cargo clippy --workspace --all-targets` never goes red mid-branch.

**IMPORTANT — the allow list below is REASONED from clippy's documented behavior (this is authoring; do not run cargo before Step 2). Treat it as the EXPECTED set: after Step 2, add an allow to WHATEVER clippy actually flags in `wordcartel-core`. If clippy flags a site NOT listed here, or does not flag one that IS listed, STOP and surface the discrepancy — do not silently diverge.**

**Files:**
- Modify: `wordcartel-core/src/lib.rs` (crate attribute), `wordcartel-core/src/block_tree.rs` (2 allows), `wordcartel-core/src/theme.rs` (4 allows).

**Interfaces:** none changed — attributes only.

- [ ] **Step 1: Add the gate attribute.** In `wordcartel-core/src/lib.rs`, immediately after `#![forbid(unsafe_code)]`:

```rust
#![forbid(unsafe_code)]
// H7: cast-soundness gate for the pure kernel — the offset/region math concentrates here
// and unsafe is already forbidden, so hold the higher bar. Benign widenings carry an
// item-local #[allow] + one-line reason (the reason-carrying-allow idiom used for
// too_many_lines / print_stdout). The shell is deliberately NOT gated (~70 terminal-
// coordinate casts stay unannotated).
#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)]
```

- [ ] **Step 2: Run clippy to see the gate fire**

Run: `cargo clippy -p wordcartel-core --all-targets`
Expected: FAIL — cast-lint errors at the PRODUCTION sites enumerated in Step 3 (block_tree `Edit::delta`, `shift_offset`, `tag_to_kind`; theme `rgb_to_xterm256`, `dist2`, `blend`, `hsl_to_rgb`) AND the one TEST-side site in Step 3b (block_tree `f4_splice_moves_before_and_shifts_after_without_reallocating`, `delta as usize` → `cast_sign_loss`; `--all-targets` lints `#[cfg(test)]` code too). Record the exact list clippy prints; reconcile against Step 3 + 3b.

- [ ] **Step 3: Add the item-local allows.** Place each `#[allow(...)]` on the line ABOVE the enclosing `fn` (attributes attach to the fn item; one allow on a fn covers every cast inside it).

`wordcartel-core/src/block_tree.rs`:

- On `Edit::delta` (locate `pub fn delta(&self) -> isize` inside `impl Edit`):
```rust
    #[allow(clippy::cast_possible_wrap)] // usize offsets are <= the ~5 MB doc cap (change.rs policy), ~12 orders below isize::MAX
    pub fn delta(&self) -> isize {
```
- On `shift_offset` (the fn added in T1):
```rust
#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)] // same doc-cap bound; .max(0) makes the isize->usize cast non-negative
fn shift_offset(pos: usize, delta: isize) -> usize {
```
- On `tag_to_kind` (locate `fn tag_to_kind(tag: &Tag) -> Option<BlockKind>`; the flagged cast is `*level as usize as u8` on the `Tag::Heading` arm):
```rust
#[allow(clippy::cast_possible_truncation)] // markdown heading level is 1..=6, always fits u8
fn tag_to_kind(tag: &Tag) -> Option<BlockKind> {
```

`wordcartel-core/src/theme.rs`:

- On `rgb_to_xterm256` (flagged cast `((r as u16 + g as u16 + b as u16) / 3) as u8`):
```rust
#[allow(clippy::cast_possible_truncation)] // channel average of three u8s / 3 is 0..=255, fits u8
fn rgb_to_xterm256(r: u8, g: u8, b: u8) -> u8 {
```
- On `dist2` (flagged cast `(v * v) as u32`):
```rust
#[allow(clippy::cast_sign_loss)] // v*v is a non-negative square
fn dist2(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
```
- On `blend` (flagged cast `.clamp(0.0, 255.0) as u8`):
```rust
#[allow(clippy::cast_possible_truncation)] // .clamp(0.0, 255.0) bounds the f32 before the u8 cast
fn blend(base: Color, pole: (u8, u8, u8), pct: f32) -> Color {
```
- On `hsl_to_rgb` (flagged casts `(l * 255.0).round() as u8` and the three `(hue_to_rgb(...) * 255.0).round() as u8`):
```rust
#[allow(clippy::cast_possible_truncation)] // hue_to_rgb outputs are in [0,1]; *255 rounds into 0..=255, fits u8
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
```

**Confirm no allow is needed at these PRODUCTION widenings (u8 -> larger int; the gate does not flag them):** `theme.rs` `A[i as usize]` (in `xterm256_to_rgb`), `theme.rs` `(n.clamp(1, 6) - 1) as usize` (both `Theme::face` and `Theme::face_mut`, `SemanticElement::Heading(u8)`), and `search.rs` `(d - b'0') as usize` (`d: u8`). If clippy flags any of these, STOP and surface it — the reasoning (widening cannot truncate/lose sign/wrap) says it should not.

- [ ] **Step 3b: Fix the TEST-side casts the gate flags (`--all-targets` lints `#[cfg(test)]` code).**

A crate-scan of every `wordcartel-core` `#[cfg(test)]` region finds exactly ONE cast the three lints flag — rewrite it (no test-local allow needed):

- `wordcartel-core/src/block_tree.rs`, test `f4_splice_moves_before_and_shifts_after_without_reallocating` (locate by name), the `assert_eq!` comparing the after-block span. Current line:
```rust
        assert_eq!(t.root.children[new_after_idx].span,
            (after_span0.start + delta as usize)..(after_span0.end + delta as usize),
            "after-block span not shifted by delta");
```
`delta` is bound `let delta = 1isize;` just above, and `delta as usize` (isize -> usize, ×2) trips `clippy::cast_sign_loss`. Rewrite to use the `shift_offset` helper (in scope via `use super::*`) — behavior-identical (it computes `pos + delta` with the same clamp) and it dogfoods the helper, so no cast and no allow:
```rust
        assert_eq!(t.root.children[new_after_idx].span,
            shift_offset(after_span0.start, delta)..shift_offset(after_span0.end, delta),
            "after-block span not shifted by delta");
```

**Confirm these two other test-side casts do NOT fire (widenings; no allow, no rewrite):** `wordcartel-core/src/history.rs` `clock.set(i as u64 * 100)` (`i` is a `usize` loop index from `.enumerate()`; `usize -> u64` never truncates/loses sign/wraps) and `wordcartel-core/src/theme.rs` `r as u32 + g as u32 + b as u32` in the test-only `lum` closure (`u8 -> u32` widening). If clippy flags either, STOP and surface it.

**Revised STOP-on-divergence rule (production AND test):** after Step 2, add an allow (or the Step-3b rewrite) to exactly the sites clippy prints. STOP and surface ONLY if (a) a PRODUCTION site diverges from the Step-3 expected set, or (b) a TEST-side site appears that is NOT `block_tree.rs`'s `f4_splice_…` line enumerated in Step 3b. A test-side site already listed here is EXPECTED, not a stop. The gate attribute (Step 1), all production allows (Step 3), and the test-side rewrite (Step 3b) land together in the single Step-5 commit so `cargo clippy --workspace --all-targets` never goes red mid-branch.

- [ ] **Step 4: Run clippy + tests to verify clean**

Run: `cargo clippy --workspace --all-targets`
Expected: clean (zero warnings/errors). Then `cargo test -p wordcartel-core` — still green: the production allows change no behavior, and the Step-3b rewrite is behavior-identical (`shift_offset(x, 1) == x + 1`), so `f4_splice_moves_before_and_shifts_after_without_reallocating` still passes.

- [ ] **Step 5: Commit**

```bash
cd /home/jkeim/projects/groundwords
git add wordcartel-core/src/lib.rs wordcartel-core/src/block_tree.rs wordcartel-core/src/theme.rs
git commit -m "chore(core): H7 — deny pedantic cast lints in wordcartel-core + reason-carrying allows

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG"
```

---

### Task 3: `build_multi_replace` well-formedness guard (commands.rs)

Add one up-front guard: a malformed edit list (empty, non-`from<=to`, non-ascending/overlapping, or out-of-bounds) degrades to the identity no-op pair BEFORE any ops are built — so `ChangeSet::from_ops`'s release `assert!(retain + delete == len_before, …)` is never reached on malformed input. This subsumes the two previously-flagged `edits.first()/.last().unwrap()` panics (they now sit behind the guard on the validated path). No production caller ever hits the guard (all four pass ascending, non-overlapping spans); it is boundary insurance for Effort P.

**Files:**
- Modify: `wordcartel/src/commands.rs` — `pub fn build_multi_replace` (locate by name).
- Test: `wordcartel/src/commands.rs` (existing `#[cfg(test)] mod tests`, `use super::*` — the existing `multi_replace_builds_one_changeset_covering_all` test is the style anchor).

**Interfaces:**
- Unchanged signature: `pub fn build_multi_replace(edits: &[(usize, usize, String)], doc_len: usize) -> (wordcartel_core::change::ChangeSet, wordcartel_core::block_tree::Edit)`.
- Behavior added: a malformed `edits` returns `(no-op ChangeSet, Edit { range: 0..0, new_len: 0 })`.

- [ ] **Step 1: Write the failing tests.** Append inside `mod tests` in `wordcartel/src/commands.rs` (near `multi_replace_builds_one_changeset_covering_all`):

```rust
    #[test]
    fn multi_replace_empty_list_is_identity_noop() {
        let (cs, edit) = super::build_multi_replace(&[], 5);
        assert_eq!(edit.range, 0..0);
        assert_eq!(edit.new_len, 0);
        let mut tb = wordcartel_core::buffer::TextBuffer::from_str("hello");
        cs.apply(&mut tb);
        assert_eq!(tb.slice(0..tb.len()), "hello"); // no-op apply leaves the doc unchanged
    }

    #[test]
    fn multi_replace_reversed_pair_is_identity_noop_not_panic() {
        // (from=10, to=5) would over-consume the doc (retain 10 then retain doc_len-5)
        // and trip ChangeSet::from_ops' release assert. The guard degrades it to a no-op.
        let (cs, edit) = super::build_multi_replace(&[(10, 5, "x".into())], 20);
        assert_eq!(edit.range, 0..0);
        assert_eq!(edit.new_len, 0);
        let mut tb = wordcartel_core::buffer::TextBuffer::from_str("abcdefghijklmnopqrst"); // 20
        cs.apply(&mut tb);
        assert_eq!(tb.slice(0..tb.len()), "abcdefghijklmnopqrst");
    }

    #[test]
    fn multi_replace_overlapping_list_is_identity_noop() {
        // second edit starts (3) before the first ends (4) -> not ascending/non-overlapping.
        let (cs, edit) = super::build_multi_replace(&[(0, 4, "x".into()), (3, 6, "y".into())], 10);
        assert_eq!(edit.range, 0..0);
        let mut tb = wordcartel_core::buffer::TextBuffer::from_str("0123456789");
        cs.apply(&mut tb);
        assert_eq!(tb.slice(0..tb.len()), "0123456789");
    }

    #[test]
    fn multi_replace_out_of_bounds_is_identity_noop() {
        // last_to (12) exceeds doc_len (10).
        let (cs, edit) = super::build_multi_replace(&[(0, 12, "x".into())], 10);
        assert_eq!(edit.range, 0..0);
        let mut tb = wordcartel_core::buffer::TextBuffer::from_str("0123456789");
        cs.apply(&mut tb);
        assert_eq!(tb.slice(0..tb.len()), "0123456789");
    }

    #[test]
    fn multi_replace_valid_ascending_still_builds_covering_edit() {
        // Regression: the guard must NOT reject well-formed input.
        let (cs, edit) = super::build_multi_replace(
            &[(0, 2, "b".into()), (3, 5, "b".into()), (6, 8, "b".into())], 8);
        let mut tb = wordcartel_core::buffer::TextBuffer::from_str("aa aa aa");
        cs.apply(&mut tb);
        assert_eq!(tb.slice(0..tb.len()), "b b b");
        assert_eq!(edit.range, 0..8);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p wordcartel -- multi_replace_empty_list_is_identity_noop multi_replace_reversed_pair_is_identity_noop_not_panic multi_replace_overlapping_list_is_identity_noop multi_replace_out_of_bounds_is_identity_noop multi_replace_valid_ascending_still_builds_covering_edit`
Expected: the four malformed-input tests FAIL by PANIC — but by DIFFERENT mechanisms in the CURRENT pre-guard body (which opens with `debug_assert!(!edits.is_empty())` and computes the `let delta: isize = …(t - f) as isize…` fold BEFORE `ChangeSet::from_ops`):
- `multi_replace_empty_list_is_identity_noop` → panics at `debug_assert!(!edits.is_empty())` (fires under `cargo test` before `.first().unwrap()` is reached).
- `multi_replace_reversed_pair_is_identity_noop_not_panic` `(10, 5)` → panics at the `(t - f)` usize subtraction underflow in the delta fold (debug overflow panic), BEFORE reaching `from_ops`.
- `multi_replace_overlapping_list_is_identity_noop` and `multi_replace_out_of_bounds_is_identity_noop` → reach and trip `ChangeSet::from_ops`' `retain + delete == len_before` release assert.

`multi_replace_valid_ascending_still_builds_covering_edit` PASSES (unchanged behavior on valid input).

- [ ] **Step 3: Implement the guard.** Replace the body of `pub fn build_multi_replace` (locate by name; current body starts with `debug_assert!(!edits.is_empty());`). The `debug_assert!(!edits.is_empty())` is REMOVED — the guard replaces it (a retained debug_assert would itself panic the malformed-input tests under `cargo test`):

```rust
pub fn build_multi_replace(
    edits: &[(usize, usize, String)],
    doc_len: usize,
) -> (wordcartel_core::change::ChangeSet, wordcartel_core::block_tree::Edit) {
    use wordcartel_core::change::{ChangeSet, Op, Tendril};
    // WELL-FORMEDNESS GUARD (H7): the op-builder below assumes `edits` is a non-empty,
    // ascending, non-overlapping, in-bounds sequence. A malformed list would make the ops
    // over-/under-consume the document and trip ChangeSet::from_ops' release assert. Any
    // violation degrades to the identity no-op — a malformed multi-replace does NOTHING, so
    // it can neither panic nor corrupt. No production caller hits this (all pass ascending,
    // non-overlapping spans); it is the boundary insurance Effort P will lean on.
    let well_formed = !edits.is_empty()
        && edits.iter().all(|(f, t, _)| f <= t)
        && edits.windows(2).all(|w| w[0].1 <= w[1].0)
        && edits.last().is_some_and(|(_, t, _)| *t <= doc_len);
    if !well_formed {
        let ops = if doc_len > 0 { vec![Op::Retain(doc_len)] } else { Vec::new() };
        let cs = ChangeSet::from_ops(ops, doc_len);
        let edit = wordcartel_core::block_tree::Edit { range: 0..0, new_len: 0 };
        return (cs, edit);
    }
    let mut ops = Vec::new();
    let mut pos = 0usize;
    for (from, to, text) in edits {
        if *from > pos { ops.push(Op::Retain(from - pos)); }
        if to > from { ops.push(Op::Delete(to - from)); }
        if !text.is_empty() { ops.push(Op::Insert(Tendril::from(text.as_str()))); }
        pos = *to;
    }
    if doc_len > pos { ops.push(Op::Retain(doc_len - pos)); }
    // The guard proved edits non-empty + ascending + in-bounds, so first<=last_to<=doc_len
    // and every subtraction below is non-negative — no saturating dressing needed.
    let first = edits.first().unwrap().0;
    let last_to = edits.last().unwrap().1;
    // new_len of the covering region = (last_to - first) adjusted by all deltas.
    let delta: isize = edits.iter().map(|(f, t, s)| s.len() as isize - (t - f) as isize).sum();
    let new_len = ((last_to - first) as isize + delta) as usize;
    let cs = ChangeSet::from_ops(ops, doc_len);
    let edit = wordcartel_core::block_tree::Edit { range: first..last_to, new_len };
    (cs, edit)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p wordcartel -- multi_replace_empty_list_is_identity_noop multi_replace_reversed_pair_is_identity_noop_not_panic multi_replace_overlapping_list_is_identity_noop multi_replace_out_of_bounds_is_identity_noop multi_replace_valid_ascending_still_builds_covering_edit`
Expected: PASS (5 tests). Then `cargo test -p wordcartel` — all shell tests PASS (the four production callers pass well-formed lists, so no behavior change). Then `cargo clippy --workspace --all-targets` — clean (the shell is not cast-gated; `is_some_and` avoids the `map_or(false, …)` clippy lint).

- [ ] **Step 5: Commit**

```bash
cd /home/jkeim/projects/groundwords
git add wordcartel/src/commands.rs
git commit -m "fix(commands): H7 — build_multi_replace degrades malformed edit lists to a no-op

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG"
```

---

### Task 4: Cursor-narrowing clamps (render.rs `place_cursor` + nav.rs `screen_pos`)

Two silent saturating fixes — a mis-clamped cursor is cosmetic, so NO `debug_assert` here. In `place_cursor`, compute the status-row column in `usize` and guard against the width BEFORE narrowing (a >65535-column field must HIDE the caret, not truncate to a small in-range column that wrongly passes the guard). In `screen_pos`, narrow `vcol` saturating so the existing D2 clamp in `place_cursor` pins the caret at the text edge if it is ever off the far right.

**Files:**
- Modify: `wordcartel/src/render.rs` — `fn place_cursor` (locate by name; the `editor.search` arm and the `editor.minibuffer` arm).
- Modify: `wordcartel/src/nav.rs` — `pub fn screen_pos` (locate by name; the final `Some((vcol as u16, final_row as u16))`).
- Test: `wordcartel/src/render.rs` (existing `mod tests`, using the `render_capturing_cursor` helper).

**Interfaces:** both signatures unchanged (`place_cursor(frame, editor, area, edit_top, edit_height, status_row, tg)`, `screen_pos(editor) -> Option<(u16, u16)>`).

- [ ] **Step 1: Write the failing test.** Append inside `mod tests` in `wordcartel/src/render.rs` (near the other `render_capturing_cursor` tests):

```rust
    #[test]
    fn place_cursor_minibuffer_hides_caret_past_terminal_width_no_wraparound() {
        // A minibuffer answer longer than u16::MAX chars must HIDE the caret (its column is
        // off-screen), not truncate the column into the visible range. Pre-H7 the
        // `chars().count() as u16` truncated FIRST, so a length of 65536+10 wrapped to
        // column 10 and wrongly passed the `< w` guard, planting the caret at col ~12.
        let mut e = Editor::new_from_text("body\n", None, (80, 24));
        e.open_minibuffer("x ", crate::minibuffer::MinibufferKind::SaveAs);
        {
            let mb = e.minibuffer.as_mut().unwrap();
            mb.text = "a".repeat(65_546); // 65536 + 10
            mb.cursor = mb.text.len();
        }
        let cur = render_capturing_cursor(&mut e, 80, 24);
        // A suppressed cursor shows as the TestBackend default (0, 0), NOT (~12, status_row).
        assert_eq!(cur, Some((0, 0)),
            "a caret past the terminal width must be hidden, not wrapped into view");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p wordcartel -- place_cursor_minibuffer_hides_caret_past_terminal_width_no_wraparound`
Expected: FAIL — assertion `left == right` mismatch: `left: Some((12, 23))` (the truncated column 10 + 2-char prompt, on the status row) vs `right: Some((0, 0))`. (A failing assertion, not a compile error — the test uses only existing symbols.)

- [ ] **Step 3: Implement the clamps.**

(a) `wordcartel/src/render.rs`, `fn place_cursor` — the `editor.search` arm. Replace the `x_offset` computation + guard (keep the existing `prefix_cols`/`caret_cols` lines and the leading comment):

```rust
        let caret_cols = s.focused_field()[..s.cursor].chars().count();
        // H7: sum in usize and guard BEFORE narrowing — a >65535-column field must hide
        // the caret, not truncate to a small column that passes the `< w` guard.
        let x_offset = prefix_cols + caret_cols;
        if x_offset < w as usize {
            frame.set_cursor_position(Position { x: area.x + x_offset as u16, y: status_row });
        }
```

(b) Same fn — the `editor.minibuffer` arm. Replace the `prompt_cols`/`text_cols`/`caret_col` computation + guard. Keep the two comment lines about using char counts for multibyte text, but DROP the now-stale trailing "(small strings, safe)" clause (the guard, not smallness, is what makes it safe):

```rust
        let prompt_cols = mb.prompt.chars().count();
        let text_cols = mb.text[..mb.cursor].chars().count();
        // H7: sum in usize and guard BEFORE narrowing (see the search arm).
        let caret_col = prompt_cols + text_cols;
        if caret_col < w as usize {
            frame.set_cursor_position(Position { x: area.x + caret_col as u16, y: status_row });
        }
```

Leave the normal-caret arm (the `nav::screen_pos` branch) unchanged except that it benefits from (c); its `(tg.text_width as usize).saturating_sub(1) as u16` is already bounded (`text_width` is a u16) — do not touch it.

(c) `wordcartel/src/nav.rs`, `pub fn screen_pos` — the final return. Narrow `vcol` saturating (leave `final_row`, which is already bounded by `final_row < area_height <= u16::MAX`):

```rust
    Some((u16::try_from(vcol).unwrap_or(u16::MAX), final_row as u16))
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p wordcartel -- place_cursor_minibuffer_hides_caret_past_terminal_width_no_wraparound`
Expected: PASS (1 test). Then `cargo test -p wordcartel` — all shell tests PASS (the existing `render.rs`/`nav.rs` cursor tests use in-range values, unchanged by saturating casts). Then `cargo clippy --workspace --all-targets` — clean.

- [ ] **Step 5: Commit**

```bash
cd /home/jkeim/projects/groundwords
git add wordcartel/src/render.rs wordcartel/src/nav.rs
git commit -m "fix(render,nav): H7 — clamp cursor-column narrowing instead of truncating

Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG"
```

---

## Final verification (before declaring the effort done)

- [ ] `cargo test` green across ALL suites: `cargo test -p wordcartel-core` (lib + F2 oracle) and `cargo test -p wordcartel` (lib).
- [ ] `cargo build` + `cargo test --no-run -p wordcartel-core -p wordcartel` warning-free.
- [ ] `cargo clippy --workspace --all-targets` clean — the merge GATE, now including the core cast-lint gate from T2.
- [ ] `cargo test -p wordcartel --test module_budgets` green (`render.rs` still under 900 production lines — T4 adds ~4).
- [ ] PTY smoke suite: `scripts/smoke/run.sh` — mandatory-run / advisory-pass; quote its one-line summary verbatim in the pre-merge report (a red result is surfaced, not a blocker).

## Spec-requirement coverage map

| Spec section | Requirement | Task |
|---|---|---|
| §4.1/§4.2 | `shift_offset` helper; route 8 core sites; band-clamp sites 3/4; clamp-at-0 site 8; keep `.min(len)` at 5–7; `Edit::delta` untouched | T1 |
| §4.3a | `build_multi_replace` up-front well-formedness guard → identity no-op; remove `debug_assert!(!edits.is_empty())`; subsumes the 2 UNCLEAR unwraps | T3 |
| §4.3b | `place_cursor` compute-in-usize, guard-before-narrow (search + minibuffer arms), no debug_assert | T4 |
| §4.3c | `screen_pos` saturating vcol narrow so the D2 clamp pins the caret, no debug_assert | T4 |
| §5/§6 | Panic-verification verdict — guarded unwraps/expects/gated panic! left as-is (non-goal); only the `build_multi_replace` fix | T3 (+ nothing else, by construction) |
| §7 | Core-crate `#![deny(cast_*)]` gate + item-local allows; shell NOT gated | T2 |
| §9 | Guardrail (clamp-not-wrap) tests; `build_multi_replace` regression tests (empty/reversed/overlapping/oob + valid); cursor-clamp test; F2 oracle unchanged; cargo test + workspace clippy merge gate | T1, T3, T4 + Final verification |

## History

- 2026-07-10 — drafted (Fable, H7 authoring thread) for Codex plan review.
- 2026-07-10 — rev 2 after Codex NO-GO (no Critical; two findings folded):
  - **T2:** the crate-level `#![deny(cast_*)]` + `--all-targets` also lints `#[cfg(test)]` code, so the allow-list now enumerates the test side. A crate-scan found exactly ONE test-side cast the gate flags — `block_tree.rs` test `f4_splice_moves_before_and_shifts_after_without_reallocating`'s `delta as usize` (isize→usize, `cast_sign_loss`) — fixed by a `shift_offset` rewrite (no cast, dogfoods the helper) in new Step 3b; the two other test-side casts (`history.rs` `i as u64`, `theme.rs` `r as u32`) are widenings that do not fire and are confirmed no-op. The STOP-on-divergence rule now treats an already-enumerated test-side site as expected, stopping only on an unlisted production OR test site.
  - **T3 Step 2:** corrected the expected red-state MECHANISM per case against the current pre-guard body (empty → `debug_assert!(!edits.is_empty())`; reversed `(10,5)` → `(t - f)` usize-underflow panic in the delta fold; overlapping + out-of-bounds → `ChangeSet::from_ops` release assert). All four still fail pre-guard and pass post-guard.
  - Codex adjudicated the §4.3c `screen_pos` reachability claim SOUND (layout grapheme-wraps even unbroken 65 KB text, so vcol never nears u16::MAX); the defensive-only, no-dedicated-test framing for the `screen_pos` clamp is retained by design.
