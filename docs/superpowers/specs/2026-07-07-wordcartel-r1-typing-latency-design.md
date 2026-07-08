# R1 — Typing-latency fix: eliminate the per-keystroke O(document) outline/fold walks

**Status:** design (brainstorm-approved 2026-07-07)
**Branch:** `effort-r1-typing-latency` (already holds the investigation record + the burst bench)
**Scope:** shell-only (`wordcartel/src`) — `derive.rs`, `fold.rs`, `app.rs` — no `wordcartel-core` change, no new crates.

---

## Command-surface contract conformance

**N/A — this effort does not touch the command surface.** It changes no commands, user-settable
options, palette entries, menu entries, or keybindings. No `docs/design/command-surface-contract.md`
invariants are engaged.

---

## 1. Problem (verified + quantified)

`derive::rebuild_downstream` (`wordcartel/src/derive.rs`) runs on every keystroke and performs **two
whole-block-tree walks** even when zero folds are active:

1. **Fold-anchor reconcile** — `outline::heading_starts(blocks, rope)` (called at ~derive.rs:243), a
   pre-order block-tree traversal that builds a full `Vec<Heading>` (`ordered`→`headings`), allocating a
   `String` title per heading via `heading_title` (`outline.rs:50-63`), then keeps only the byte offsets
   and discards the titles — feeding `folds.reconcile_to(&starts)`.
2. **Fold-view compute** — `active_fold_view()` (called at ~derive.rs:253) → `FoldView::compute`
   (`fold.rs:133`) → `outline::sections(blocks, rope)` (`outline.rs:109`), a second full traversal that
   clones a `Heading` + allocates a `String` per heading, **then** filters by `folds.folded`.

Both are gated on `blocks_generation`, which `Editor::set_blocks` (`editor.rs:91-94`) bumps
**unconditionally on every edit**, so the memoization is defeated — the walks run every keystroke.

**Quantified (burst bench, branch `effort-r1-typing-latency`, `6669e73`, release, e2e seam):** p99 of
`heading_starts` and `foldview` grows **linearly with block/heading count** — slope 0.99/1.03
(heading-dense), 1.33/1.20 (nested-list), 1.31/1.31 (code-heavy); flat on flat-prose / one-block table
at the µs floor. The walk fired on **200/200 Input frames**. Positive control held: `layout_fill`
(~300 µs, the largest phase, correctly O(visible)) and `render` are flat. No cell breached the 8 ms
(120 Hz) budget at ≤1 MB (worst total p99 ~6.3 ms) — this is a **scaling + burst-backlog** cost (per-key
work stacking faster than 16 ms frames drain, given no input coalescing), not a single-key stall today.

**Why:** the code conflates "text changed" with "structure changed," and — the key observation — the
overwhelmingly common case is **no folds active**, where both walks accomplish nothing: there are no
anchors to reconcile, and the hidden-range set is empty by construction.

**Separately (T5, a correctness bug in the same path):** at startup/session-resume the sequence is
`derive::rebuild → snap caret → nav::ensure_visible → draw` (`app.rs:~1576-1590`). `ensure_visible`
mutates `view.scroll`/`scroll_row` *after* the rebuild and returns `()` with no signal, and there is no
rebuild between it and the first `draw`. If the caret is off-screen on open, the **first frame's layout
cache is built for the wrong visible range** (blank/wrong editing rows until the next keystroke repairs
it). Every path reached through the reduce loop is fine — `advance` runs a pre-draw `derive::rebuild`
every keystroke; startup is the one path that skips it.

## 2. Approach chosen (and why it is the *most correct*, not just pragmatic)

**No-folds fast path**: guard both walks on the existing `FoldState::is_empty()` predicate
(`fold.rs:19`). This is the most correct option in the sense that matters for a no-data-loss editor —
it skips **only work that is empty by construction**, never work a heuristic *guesses* is unnecessary:
- `folds.reconcile_to(&starts)` with an empty `folded` set has no anchors to prune — a literal no-op.
- `FoldView::compute` with an empty `folded` set filters `sections()` down to nothing — the result is
  already `{hidden: [], total}`.

So the fast path is **behavior-identical**, and cannot desync fold/outline state. The rejected
alternative — a *structural generation* that would also eliminate the walks when folds ARE active — is
**less** correct: it replaces empty-work-elision with change-detection that, if ever wrong, silently
staleness the fold anchors/outline (visible corruption of what's hidden), and the only reliable version
of it couples to the incremental parser's change-reporting, whose soundness the F2 fuzz oracle shows is
not yet complete. The residual gap the fast path leaves — folds-active typing stays O(headings)/keystroke
— is a **performance** gap (bounded, and a rare state), not a correctness gap. Deferred, not chased.

## 3. Components

### Component 1 — No-folds fast path (behavior-identical)

**1a. Reconcile guard** — `wordcartel/src/derive.rs::rebuild_downstream` (~derive.rs:237-247). Extend
the existing generation gate with `!folds.is_empty()`:

```rust
let gen = editor.active().document.blocks_generation();
if !editor.active().folds.is_empty()
    && editor.active().last_reconciled_generation != Some(gen)
{
    // …existing heading_starts + reconcile_to + set last_reconciled_generation…
}
```

When `folds.is_empty()`, skip the block entirely (including leaving `last_reconciled_generation` as-is —
it is only read by this gate, and when a fold is later created the block runs at the new generation).

**1b. Trivial FoldView** — `wordcartel/src/fold.rs::FoldView::compute` (~fold.rs:133). Early-return the
empty view before the `sections()` walk:

```rust
pub fn compute(folds: &FoldState, blocks: &BlockTree, buf: &TextBuffer) -> FoldView {
    let rope = buf.snapshot();
    let total = rope.len_lines();
    if folds.is_empty() {
        return FoldView { hidden: Vec::new(), total };
    }
    // …existing sections()/filter/merge…
}
```

`total` matches the value the full path computes (`rope.len_lines()`), so every `FoldView` method
(`normalize_line`/`next_visible`/`prev_visible`/`visible_count`/…) behaves identically to a computed
empty view. This fixes **all** callers — `derive` and `nav.rs`'s `fold_view(editor)` — in one place.

After 1a+1b, a no-folds keystroke's derive downstream is `layout_fill` (O(visible)) + O(log n) scalars;
the two O(document) walks are gone.

### Component 2 — Regression guard (locks the invariant in)

Add two `#[cfg(test)]` thread-local walk counters placed on the **expensive** paths only, so they count
a walk only when it actually runs:
- one incremented inside the guarded reconcile block in `derive.rs` (the `heading_starts` walk);
- one incremented inside `FoldView::compute` **after** the `is_empty` early-return (the `sections()`
  walk).

Then a test (co-located with the existing bench instrumentation in `e2e.rs`, or a focused `derive`/`fold`
test) that:
- drives a burst of char inserts into a **no-folds** document → asserts **both counters == 0** across
  the burst (the durable invariant lock);
- creates a **fold**, then edits → asserts the counters **do** increment (positive control: the guards
  skip empty work, they do not disable folding).

### Component 3 — T5 first-frame fix

`wordcartel/src/app.rs`, the startup/session-resume site (~app.rs:1589): add a rebuild after
`ensure_visible`, before the first `draw`:

```rust
crate::nav::ensure_visible(&mut editor);
derive::rebuild(&mut editor); // T5: ensure_visible may have scrolled to an off-screen caret;
                              // LayoutKey-gated, so a no-op when scroll didn't move.
guard.terminal().draw(|f| render::render(f, &mut editor))?;
```

`derive::rebuild` is `LayoutKey`-gated (`derive.rs:299-301`), so it is a cheap early-return when scroll
is unchanged and a correct refresh when `ensure_visible` moved the viewport.

## 4. Testing

- **Behavior-identical unit tests:** `FoldView::compute` empty-folds trivial-return equals a
  computed-empty view for `normalize_line`/`next_visible`/`prev_visible`/`total`; the reconcile guard
  skips when `folds.is_empty()` and still runs when folds exist.
- **Regression guard (Component 2):** the no-folds-zero-walks assertion + the folds-active positive
  control.
- **T5 test:** open/resume with the caret off-screen (scroll far from caret) → assert the first frame's
  `line_layouts` cover the caret's visible range (matching the post-`ensure_visible` scroll), not the
  pre-scroll range.
- **Latency evidence (not a gate):** re-run the branch's burst bench; expect `heading_starts` and
  `foldview` p99 to drop to ~0 / flat on the no-folds cells — the before/after proof.
- **No-regression GATE:** full `wordcartel` (918) + `wordcartel-core` (278) suites green; workspace
  clippy clean; the guards must not change any rendered output (behavior-identical). PTY smoke
  mandatory-run / advisory-pass.

## 5. Files touched (map)

- `wordcartel/src/derive.rs` — reconcile guard (`!folds.is_empty()`); the `#[cfg(test)]` heading-starts
  walk counter.
- `wordcartel/src/fold.rs` — `FoldView::compute` `is_empty` early-return; the `#[cfg(test)]` sections
  walk counter.
- `wordcartel/src/app.rs` — the T5 `derive::rebuild` after `ensure_visible` at the startup site.
- Tests co-located in the above files and/or `wordcartel/src/e2e.rs` (reusing the existing bench
  instrumentation seam).
- No `wordcartel-core` change; no `Cargo.toml` change; no new crates.

## 5b. Component 4 — `normalize_caret` guard (folded in 2026-07-07, post-Fable)

The Fable whole-branch gate found a THIRD instance of the same walk, outside `rebuild_downstream`:
`fold::normalize_caret` (fold.rs:267) calls `outline::heading_starts` **unconditionally**, and it sits on
the per-keystroke path for **caret navigation** — every arrow key (`Command::Move` central normalize),
undo/redo/shrink (`place_caret_visible` SnapOut), mouse click, and `move_doc_end`. So a no-folds document
still pays an O(document) walk on caret movement — which maps to the user's **line-jump (symptom 2)**, not
just the typing jerk. **Folded into this effort (human decision 2026-07-07)** because it is the identical
bug and an identical, behavior-identical fix.

**Fix:** add `if folds.is_empty() { return byte; }` at the top of `normalize_caret` (fold.rs:273), before
the `heading_starts` walk. Behavior-identical: with an empty `folded` set the existing loop over
`folds.folded` never executes and returns `byte` unchanged, so the guard produces the same result while
skipping the walk. Plus a `#[cfg(test)]` counter (mirroring Tasks 1-2) asserting zero walks on the
no-folds path and ≥1 when a fold is active. (Plan Task 4.)

## 6. Out of scope (recorded, deliberate)

- **Reconcile-debounce retiming** — the bench proved the reconcile full-parse runs off-thread in
  production (threaded `Executor`; merge Tick flat ~78-146 µs); the 150 ms-vs-cadence concern is moot on
  the hot path. Not touched.
- **Structural-generation / folds-active walk elimination** — rejected as less correct (heuristic
  invalidation can desync folds; reliable version couples to the not-yet-sound incremental parser).
  Deferred; revisit only with its own before/after numbers if folds-active latency ever proves real.
- **Input coalescing** — deferred; touches the input loop + the no-silent-UI invariant. Recorded as a
  real burst-backlog contributor (bench-supported), revisit if typing still feels bursty after this fix.
- **Incremental-soundness divergences** — the F2 oracle tail (fresh repros saved 2026-07-07); a separate
  hardening item that shares the `block_tree.rs` hotspot but is not this effort.
