# Editing responsiveness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cut the always-on per-keystroke draw-path cost — eliminate ~5–6 redundant outline/fold tree walks + the block-tree deep clone, make `role_at` O(log N), and run the visible-line layout pass at most once per actual change — with identical rendered output.

**Architecture:** Shell-mostly + one core fn. (F3) `role_at` binary search in core. (F2) a shared `Rc<FoldView>` cached on `Buffer` behind `RefCell`, keyed by a new `blocks_generation` + a `FoldState.epoch`, with `reconcile` decoupled into a generation-gated step. (Component 3) gate the layout loop on a computed `LayoutKey`, holding the output invariant via `Buffer::invalidate_layout()`.

**Tech Stack:** Rust, `wordcartel-core` + `wordcartel` shell.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-02-wordcartel-editing-responsiveness-design.md` (Codex READY ×4 + Fable5 ×2).
- **No observable behavior change** — identical rendered output; the existing render/nav/fold/layout suite is the primary net.
- Gates: `cargo test -p wordcartel -p wordcartel-core` green; `cargo build`/`test --no-run` warning-free; **`cargo clippy --workspace --all-targets` clean (deny gate LIVE)**; NO `cargo fmt`; house style (em-dash `—`).
- `#![forbid(unsafe_code)]` in core unchanged. `RefCell`/`Rc` are safe; `Buffer` is main-thread-only (jobs capture rope snapshots, never `Buffer`).
- Hot path stays O(visible)+O(edited); no task introduces O(document) work.
- Trailers on every commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

## File Structure

- `wordcartel-core/src/block_tree.rs` — F3: `collect_role` binary-search descent (+ a differential test, an ordered/non-overlap test).
- `wordcartel/src/editor.rs` — `Document.blocks_generation`; `Buffer.fold_view_cache`/`last_reconciled_generation`/`layout_key`; `Buffer::invalidate_layout`; `Editor::active_fold_view`; route the apply/undo/redo fold mutations through epoch-bumping helpers.
- `wordcartel/src/fold.rs` — `FoldState.epoch` + epoch-bumping mutators (`toggle`/`fold_all`/`unfold_all`/`reconcile_to`/`remove`/`replace_folded`/`clamp`).
- `wordcartel/src/derive.rs` — bump `blocks_generation` at :135; generation-gated reconcile; route FoldView through `active_fold_view`; the `LayoutKey` gate on the layout loop.
- `wordcartel/src/reconcile.rs` — bump `blocks_generation` at the merge (:65).
- `wordcartel/src/nav.rs` — `fold_view` returns `Rc<FoldView>` via `active_fold_view`.
- `wordcartel/src/render.rs`, `mouse.rs` — route the scrollbar FoldView computes through `active_fold_view`.
- `wordcartel/src/app.rs`, `save.rs`, `registry.rs` — route external `line_layouts` clears through `invalidate_layout`; route fold-set mutations through the epoch helpers.

---

### Task 1: F3 — `role_at` binary search (core)

**Files:** Modify `wordcartel-core/src/block_tree.rs`.

**Interfaces:** `role_at`/`collect_role` keep their signatures; only `collect_role`'s child-selection changes. Relies on: `Block.span: Range<usize>`, `Block.children: Vec<Block>` in document order, non-overlapping at every level.

- [ ] **Step 1: Add the differential + invariant tests** (in `block_tree.rs`'s `#[cfg(test)] mod tests`; reuse the oracle's `block_strategy()` random-markdown → `full_parse` approach — see `tests/block_tree_oracle.rs`):

```rust
    // Reference: the pre-change linear scan, kept for the differential test.
    fn collect_role_linear(block: &Block, byte: usize, best: &mut crate::style::BlockRole) {
        if !block.span.contains(&byte) { return; }
        if let Some(role) = kind_to_role(&block.kind) {
            if role_precedence(&role) < role_precedence(best) { *best = role; }
        }
        for child in &block.children { collect_role_linear(child, byte, best); }
    }
    fn role_at_linear(t: &BlockTree, byte: usize) -> crate::style::BlockRole {
        let mut best = crate::style::BlockRole::Paragraph;
        collect_role_linear(&t.root, byte, &mut best);
        best
    }
    fn assert_children_ordered_nonoverlapping(b: &Block) {
        let mut prev_end = 0usize;
        for c in &b.children {
            assert!(c.span.start >= prev_end, "children must be ordered + non-overlapping: {:?}", b.span);
            prev_end = c.span.end.max(prev_end);
            assert_children_ordered_nonoverlapping(c);
        }
    }

    proptest::proptest! {
        #[test]
        fn role_at_binary_matches_linear(text in crate::block_tree::tests::doc_text_strategy()) {
            let t = full_parse(&text);
            assert_children_ordered_nonoverlapping(&t.root);
            let len = text.len();
            // sample every byte for small docs; strategy keeps them small.
            for byte in 0..=len {
                prop_assert_eq!(t.role_at(byte), role_at_linear(&t, byte), "byte {}", byte);
            }
        }
    }
```
(If a `doc_text_strategy`/`block_strategy` already exists in the oracle/this module, reuse it — the extraction confirms `block_tree_oracle.rs:284` has one; if it lives in the oracle test crate only, add a small local `proptest::collection`-based markdown-ish string strategy here. The point: trees MUST come from `full_parse`, never hand-built.)

- [ ] **Step 2: Run to verify the invariant test + baseline pass** — `cargo test -p wordcartel-core role_at_binary_matches_linear` → PASS (both sides linear today; this guards the refactor).

- [ ] **Step 3: Replace `collect_role`'s child loop with a binary-search descent** (`block_tree.rs:231`):

```rust
fn collect_role(block: &Block, byte: usize, best: &mut crate::style::BlockRole) {
    if !block.span.contains(&byte) {
        return;
    }
    if let Some(role) = kind_to_role(&block.kind) {
        if role_precedence(&role) < role_precedence(best) {
            *best = role;
        }
    }
    // Children are in document order and non-overlapping, so at most one contains
    // `byte`. Find the first child whose span ends after `byte` (partition_point on
    // `span.end <= byte`); recurse only if it also starts at/before `byte`.
    let idx = block.children.partition_point(|c| c.span.end <= byte);
    if let Some(child) = block.children.get(idx) {
        if child.span.start <= byte {
            collect_role(child, byte, best);
        }
    }
}
```
(This is equivalent to the linear scan under the invariant: the linear scan recurses into every child, but only the one containing `byte` does any work; zero-length synthetic blocks have `end == start`, so `end <= byte` filters them and `start <= byte` rejects them — matching `contains`.)

- [ ] **Step 4: Run** — `cargo test -p wordcartel-core` green (the differential test now exercises binary ≡ linear; the F2 oracle in `tests/block_tree_oracle.rs` stays green). `cargo clippy -p wordcartel-core --all-targets` clean.

- [ ] **Step 5: Commit**
```bash
git add wordcartel-core/src/block_tree.rs
git commit -m "perf(core): role_at binary-search descent (O(log N) per line, not O(N_blocks))"   # + trailers
```

---

### Task 2: F2 — shared cached `FoldView` + `blocks_generation` + reconcile decouple

**Files:** Modify `wordcartel/src/editor.rs`, `fold.rs`, `derive.rs`, `reconcile.rs`, `nav.rs`, `render.rs`, `mouse.rs`, `registry.rs`, `app.rs`, `save.rs`.

**Interfaces:**
- Produces: `Document.blocks_generation: u64`; `FoldState.epoch: u64` + mutators; `Buffer.fold_view_cache: RefCell<Option<(u64, u64, Rc<crate::fold::FoldView>)>>`, `Buffer.last_reconciled_generation: Option<u64>`; `Editor::active_fold_view(&self) -> Rc<crate::fold::FoldView>`.
- `nav::fold_view(&Editor) -> Rc<crate::fold::FoldView>`.

- [ ] **Step 1: `blocks_generation` field + bumps** — in `editor.rs` `Document` (after `version`):
```rust
    pub version: u64,
    /// Monotonic id of `blocks`: bumped on EVERY `blocks` write (parse phase +
    /// reconcile merge). Identifies the current tree across the reconcile-merge
    /// boundary (where `version` is unchanged). Keys the FoldView + layout caches.
    pub blocks_generation: u64,
```
Init `blocks_generation: 0` in `Buffer::from_text` (`editor.rs:127`, next to the other `document` field inits). Bump at the two writers — `derive.rs:135`:
```rust
        editor.active_mut().document.blocks = new_blocks;
        editor.active_mut().document.blocks_generation =
            editor.active().document.blocks_generation.wrapping_add(1);
        editor.active_mut().reconcile.blocks_version = version;
```
and `reconcile.rs:64` (inside the `!= tree` branch):
```rust
            if b.document.blocks != tree {
                b.document.blocks = tree;
                b.document.blocks_generation = b.document.blocks_generation.wrapping_add(1);
            }
```
(The parse-phase bump stays UNCONDITIONAL — do not guard it on "tree changed"; a byte-identical tree can accompany changed `line_text`.)

- [ ] **Step 2: `FoldState.epoch` + epoch-bumping mutators** — `fold.rs`. Add the field + convert every mutator to bump on real change:
```rust
#[derive(Debug, Clone, Default)]
pub struct FoldState {
    pub folded: BTreeSet<usize>,
    /// Bumped whenever `folded` changes — the fold-identity token for the FoldView cache.
    pub epoch: u64,
}

impl FoldState {
    pub fn toggle(&mut self, heading_byte: usize) {
        if !self.folded.remove(&heading_byte) { self.folded.insert(heading_byte); }
        self.epoch = self.epoch.wrapping_add(1);
    }
    pub fn fold_all(&mut self, blocks: &BlockTree, buf: &TextBuffer) {
        self.folded = outline::heading_starts(blocks, &buf.snapshot());
        self.epoch = self.epoch.wrapping_add(1);
    }
    pub fn unfold_all(&mut self) {
        if !self.folded.is_empty() { self.folded.clear(); self.epoch = self.epoch.wrapping_add(1); }
    }
    /// Prune anchors not in `starts` (decoupled reconcile). Bumps only on change.
    pub fn reconcile_to(&mut self, starts: &BTreeSet<usize>) {
        let before = self.folded.len();
        self.folded.retain(|b| starts.contains(b));
        if self.folded.len() != before { self.epoch = self.epoch.wrapping_add(1); }
    }
    /// Remove one anchor (unfold_ancestors_of). Bumps on change.
    pub fn remove(&mut self, byte: usize) {
        if self.folded.remove(&byte) { self.epoch = self.epoch.wrapping_add(1); }
    }
    /// Replace the folded set wholesale (session restore). Always bumps.
    pub fn replace_folded(&mut self, new: BTreeSet<usize>) {
        self.folded = new; self.epoch = self.epoch.wrapping_add(1);
    }
    /// Clamp anchors to `<= len` (undo/redo). Bumps on change.
    pub fn clamp(&mut self, len: usize) {
        let before = self.folded.len();
        self.folded.retain(|&b| b <= len);
        if self.folded.len() != before { self.epoch = self.epoch.wrapping_add(1); }
    }
    /// Remap anchors through a ChangeSet (Buffer::apply). Always bumps (spans shift).
    pub fn remap(&mut self, cs: &wordcartel_core::change::ChangeSet) {
        self.folded = self.folded.iter().map(|&b| wordcartel_core::change::map_pos_before(b, cs)).collect();
        self.epoch = self.epoch.wrapping_add(1);
    }
}
```
Keep the existing `reconcile(&mut self, blocks, buf)` (used by session restore `app.rs:469`) but route its retain through the same bump — reimplement it as `let starts = heading_starts(...); self.reconcile_to(&starts);`.

- [ ] **Step 3: Convert the fold-mutation call sites to the helpers.**
  - `editor.rs:203-210` `Buffer::apply` remap → `self.folds.remap(&cs);` (replace the inline `map_pos_before` block).
  - `editor.rs:235` undo, `:254` redo → `self.folds.clamp(len);`.
  - `registry.rs:509` `folds.folded.remove(&hb)` → `folds.remove(hb);`.
  - `app.rs:467` `folds.folded = entry.folds...collect()` → `folds.replace_folded(entry.folds.iter().copied().collect());`.
  - `registry.rs` toggle/fold_all/unfold_all already call the methods (now bumping) — no change.
  - `save.rs:233,:277` `folds = prev_folds` (whole `FoldState`, carries its epoch) — leave as-is; the buffer was just replaced so `fold_view_cache` is empty (miss → recompute). The plan-confirm grep must show no OTHER `folds.folded` write.

- [ ] **Step 4: Buffer cache fields + `active_fold_view` + `invalidate_layout` skeleton** — `editor.rs` `Buffer` (after `folds`):
```rust
    pub folds: crate::fold::FoldState,
    /// Memoized fold view, keyed by (blocks_generation, folds.epoch). Interior
    /// mutability so the accessor is `&self` (nav reads via `&Editor`).
    pub fold_view_cache: std::cell::RefCell<Option<(u64, u64, std::rc::Rc<crate::fold::FoldView>)>>,
    /// Generation the folded set was last reconciled (pruned) against. `None` on a
    /// fresh Buffer → the first rebuild always reconciles (covers reload/recovery).
    pub last_reconciled_generation: Option<u64>,
    /// Key `view.line_layouts` is currently valid for (Component 3, Task 3).
    pub layout_key: Option<crate::derive::LayoutKey>,
```
Init in `from_text`: `fold_view_cache: std::cell::RefCell::new(None), last_reconciled_generation: None, layout_key: None,`. (`RefCell<Option<..Rc..>>` is `Clone`+`Debug` — `Buffer` keeps `#[derive(Debug, Clone)]`.) Add the accessor on `Editor`:
```rust
    /// The active buffer's fold view, memoized by (blocks_generation, folds.epoch).
    /// Pure: never mutates document/fold state, so it takes `&self` and is usable
    /// from the `&Editor` nav helpers.
    pub fn active_fold_view(&self) -> std::rc::Rc<crate::fold::FoldView> {
        let b = self.active();
        let key = (b.document.blocks_generation, b.folds.epoch);
        if let Some((g, e, rc)) = &*b.fold_view_cache.borrow() {
            if *g == key.0 && *e == key.1 { return rc.clone(); }
        }
        let view = std::rc::Rc::new(
            crate::fold::FoldView::compute(&b.folds, &b.document.blocks, &b.document.buffer));
        *b.fold_view_cache.borrow_mut() = Some((key.0, key.1, view.clone()));
        view
    }
```
And the layout invalidator (used by Task 3, add now):
```rust
    /// Clear the visible-line layout cache AND its key — the invariant is
    /// "layout_key == Some(k) ⟹ line_layouts valid for k". Route every EXTERNAL
    /// line_layouts clear through this (Resize, reload/recovery).
    pub fn invalidate_layout(&mut self) {
        self.view.line_layouts.clear();
        self.layout_key = None;
    }
```

- [ ] **Step 5: Decouple reconcile + route FoldView in `rebuild_downstream`** (`derive.rs:162-171`). Replace the reconcile block + FoldView compute:
```rust
    // Generation-gated fold-anchor prune (was every-draw). No per-draw deep clone:
    // compute heading starts under an immutable borrow, then retain.
    {
        let gen = editor.active().document.blocks_generation;
        if editor.active().last_reconciled_generation != Some(gen) {
            let starts = {
                let b = editor.active();
                wordcartel_core::outline::heading_starts(&b.document.blocks, &b.document.buffer.snapshot())
            };
            editor.active_mut().folds.reconcile_to(&starts);
            editor.active_mut().last_reconciled_generation = Some(gen);
        }
    }
    let fold_view = editor.active_fold_view();
```
(`fold_view` is now `Rc<FoldView>`; the downstream uses `fold_view.normalize_line(...)`/`.next_visible(...)` via `Deref` — unchanged. Remove the old `blocks.clone()`/`buffer.clone()`/`FoldView::compute` lines.)

- [ ] **Step 6: Route the remaining FoldView call sites.**
  - `nav.rs:131`:
    ```rust
    fn fold_view(editor: &Editor) -> std::rc::Rc<crate::fold::FoldView> {
        editor.active_fold_view()
    }
    ```
    (The 14 callers do `let fv = fold_view(editor); fv.method()` — works through `Rc` `Deref`. No caller change.)
  - `render.rs:596` and `mouse.rs:157,:222`: replace `crate::fold::FoldView::compute(&editor.active().folds, &editor.active().document.blocks, &editor.active().document.buffer)` with `editor.active_fold_view()`.
  - `render.rs:1034` `fold_marker_for` reads `folds.folded` directly — LEAVE UNROUTED (spec plan-confirm 10; correctness-neutral).

- [ ] **Step 7: Tests** (`editor.rs`/`derive.rs` test modules):
  - `active_fold_view_reuses_rc_when_unchanged`: build editor, call `active_fold_view()` twice with no state change → `Rc::ptr_eq`.
  - `active_fold_view_recomputes_on_generation_bump`: bump `active_mut().document.blocks_generation` → new `Rc` (not ptr_eq).
  - `active_fold_view_recomputes_on_fold_toggle`: `active_mut().folds.toggle(h)` → new `Rc`.
  - `cached_foldview_equals_fresh`: `*active_fold_view() == FoldView::compute(...)` for the same state.
  - `merge_bumps_generation_invalidates`: simulate the reconcile merge (`by_id_mut → blocks = other_tree; blocks_generation bump`), assert `active_fold_view()` recomputes (regression guard for the Critical).
  - Existing reload prune test (`save.rs:671`) stays green — it exercises `last_reconciled_generation = None` (fresh buffer, blocks_version synced → parse skipped, prev_folds with a stale anchor → first rebuild reconciles).

- [ ] **Step 8: Run + gates + commit** — `cargo test -p wordcartel -p wordcartel-core` green; `cargo clippy --workspace --all-targets` clean.
```bash
git add -A
git commit -m "perf(shell): shared Rc<FoldView> cache (blocks_generation + fold_epoch) + reconcile decouple"   # + trailers
```

---

### Task 3: Component 3 — layout-pass gate via `LayoutKey`

**Files:** Modify `wordcartel/src/derive.rs` (the `LayoutKey` type + the gate), `app.rs`/`save.rs` (route external `line_layouts` clears through `invalidate_layout`).

**Interfaces:** Produces `derive::LayoutKey` (a `pub` type — referenced by `Buffer.layout_key`). Consumes `Buffer::invalidate_layout` + `blocks_generation` + `FoldState.epoch` from Task 2.

- [ ] **Step 1: The `LayoutKey` type** (`derive.rs`, near the top):
```rust
/// Everything the visible-line layout loop reads. Gate the loop on equality of this
/// so it re-runs only when an actual input changed. A miss here would blank rows
/// (render has no on-demand fallback), so the field set must be COMPLETE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutKey {
    pub blocks_generation: u64,
    pub fold_epoch: u64,
    pub scroll: usize,        // post-normalization (first_line)
    pub scroll_row: usize,
    pub area: (u16, u16),
    pub text_width: usize,    // vp_width (subsumes wrap/gutter geometry)
    pub active_line: usize,
    pub source_mode: bool,    // view.mode != LivePreview
    pub heading_level_glyph: bool,
}
```

- [ ] **Step 2: Route the external `line_layouts` clears through `invalidate_layout`.**
  - `app.rs:1735` Resize:
    ```rust
        Msg::Input(Event::Resize(w, h)) => {
            for b in editor.buffers.iter_mut() {
                b.view.area = (w, h);
                b.invalidate_layout();
            }
            derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
        }
    ```
  - `save.rs:238` (reload) and `save.rs:281` (recovery): replace `editor.active_mut().view.line_layouts.clear();` with `editor.active_mut().invalidate_layout();`.
  - `derive.rs:208`'s own clear stays (inside the gated pass, re-stores the key same pass).

- [ ] **Step 3: Failing test — same-dimension Resize must not blank** (`app.rs`/`derive.rs` tests): drive a buffer to a populated `line_layouts`; call `invalidate_layout()` (simulating the Resize clear) with the SAME `area`; run `derive::rebuild`; assert `line_layouts` is non-empty. (Fails if the gate skips on the matching key without honoring the nulled `layout_key`.)

- [ ] **Step 4: Gate the layout loop** (`derive.rs`, the section from `vp_width` at :202 through the loop at :225). Compute the key AFTER `vp_width`, compare, skip or run:
```rust
    let vp_width = crate::nav::text_geometry(editor).text_width as usize;

    let key = LayoutKey {
        blocks_generation: editor.active().document.blocks_generation,
        fold_epoch: editor.active().folds.epoch,
        scroll: first_line,
        scroll_row,
        area: editor.active().view.area,
        text_width: vp_width,
        active_line,
        source_mode,
        heading_level_glyph: editor.theme.heading_level_glyph,
    };
    if editor.active().layout_key.as_ref() == Some(&key) {
        return; // line_layouts already valid for this key — skip the pass
    }

    let mut visual_rows_accumulated: usize = 0;
    let overscan_budget = area_height.saturating_add(scroll_row).saturating_add(1);
    editor.active_mut().view.line_layouts.clear();
    #[cfg(test)] { LAYOUT_RUNS.with(|c| c.set(c.get() + 1)); }

    let mut l = first_line;
    while l < total_lines && visual_rows_accumulated < overscan_budget {
        // ... unchanged loop body (role_at, layout::layout, insert, next_visible) ...
    }
    editor.active_mut().layout_key = Some(key);
```
Add a `#[cfg(test)]` run-counter near the top of `derive.rs`:
```rust
#[cfg(test)]
thread_local! { pub static LAYOUT_RUNS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
```
(Note: `return`ing early from `rebuild_downstream` is fine — the fold-view + scroll-normalization above already ran; only the layout loop is skipped.)

- [ ] **Step 5: Tests** (`derive.rs` tests):
  - Same-dimension Resize test (Step 3) now PASSES.
  - `layout_gate_skips_when_unchanged`: reset `LAYOUT_RUNS`; `rebuild` once (runs, count 1); `rebuild` again with no state change → count stays 1 (skipped).
  - `layout_gate_reruns_on_each_input`: for each of scroll / area / text_width (via area) / active_line (caret move) / mode / a fold toggle / a `blocks_generation` bump / a `theme.heading_level_glyph` flip → the count increments. Include an explicit `heading_level_glyph`-flip-only case.
  - `keystroke_runs_layout_once` (non-scrolling, mid-screen): a single insert that doesn't scroll → exactly 1 layout run across the command rebuild + pre-draw rebuild (the double-rebuild collapsed).

- [ ] **Step 6: Run + gates + commit** — full `cargo test -p wordcartel -p wordcartel-core` green (incl. all existing render/nav/fold/layout tests — the observable-output net); `cargo clippy --workspace --all-targets` clean.
```bash
git add -A
git commit -m "perf(shell): gate the visible-line layout pass on a computed LayoutKey (collapse double-rebuild + idle Ticks)"   # + trailers
```

---

## Self-Review

**Spec coverage:** F3 binary search + differential/invariant tests → Task 1 ✓; `blocks_generation` (both writers, unconditional parse bump) → Task 2 Step 1 ✓; `FoldState.epoch` + encapsulated mutators (all 8 sites) → Task 2 Steps 2-3 ✓; `RefCell` `&self` `active_fold_view` + all 18 sites → Steps 4,6 ✓; reconcile-decouple (generation-gated, `last_reconciled_generation: Option<u64>` = None) → Step 5 ✓; `invalidate_layout` output-invariant at all external clears → Task 3 Step 2 ✓; `LayoutKey` gate (complete field set incl. `heading_level_glyph`, computed after `vp_width`) → Task 3 Steps 1,4 ✓. Tests: differential, Rc::ptr_eq reuse, merge-bumps-generation guard, reload-prune guard, same-dimension-Resize guard, layout-run counter (non-scrolling keystroke), glyph-flip invalidation ✓.

**Placeholder scan:** none — every step has complete code or a precise site edit grounded in the extraction.

**Type consistency:** `LayoutKey` (pub, in `derive`) referenced by `Buffer.layout_key: Option<crate::derive::LayoutKey>`; `active_fold_view(&self) -> Rc<FoldView>`; `nav::fold_view -> Rc<FoldView>` (Deref keeps 14 callers unchanged); `FoldState` mutators all bump `epoch`; `blocks_generation: u64` bumped at exactly two writers.

**Ordering:** F3 independent (Task 1). Task 2 introduces `blocks_generation` + `epoch` + `layout_key` field. Task 3's `LayoutKey` consumes them — correct dependency.
