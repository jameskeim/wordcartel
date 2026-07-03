# F4: In-Place Consume-and-Splice Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the per-keystroke incremental-splice ALLOCATIONS by taking `old_tree` by value and editing `root.children` in place (before-blocks moved, reparsed `Vec::splice`d in, after-blocks shifted with no per-node allocation) — a pure, byte-identical allocation-elimination refactor.

**Architecture:** Core: an owned entry `incremental_update_instrumented_src_owned` becomes the single source of truth; the existing `&`-entry delegates via `old.clone()`, so the existing oracle + F2 fuzz suite is the free byte-identical regression net. The splice (block_tree.rs:956-1003) becomes two `partition_point`s + `Vec::splice` + `shift_in_place`. Shell: `Document::take_blocks` + one `derive.rs` call site.

**Tech Stack:** Rust, `wordcartel-core` (`#![forbid(unsafe_code)]`) + `wordcartel` shell.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-03-wordcartel-f4-in-place-splice-design.md` (Codex ×2 + Fable5 folded).
- **Byte-identical behavior** — same tree, `WidenReason`, `reparsed_bytes`, and every early-return path. The existing `block_tree_oracle.rs` + F2 fuzz suite (all assert `incremental == full_parse`) is the correctness proof; NO new correctness tests.
- `#![forbid(unsafe_code)]` in core intact; NO `Block`/`BlockTree` representation change; no new dependency.
- `cargo test -p wordcartel-core -p wordcartel` green; `cargo build`/`test --no-run` warning-free; **`cargo clippy --workspace --all-targets` clean (deny gate LIVE)**; NO `cargo fmt`; house style (em-dash `—`).
- Scope: ONLY the splice — NOT the ~6 region scans, NOT the downstream `heading_starts`/`FoldView` walks, NOT F1's Case B.
- Trailers on every commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: Core — the owned entry + in-place splice

**Files:** Modify `wordcartel-core/src/block_tree.rs` (the `incremental_update_instrumented_src` signature ~:545; the splice :956-1003; `shift_block`→`shift_in_place` :1269-1275; a new `&`-wrapper + str-owned convenience).

**Interfaces produced:** `pub fn incremental_update_instrumented_src_owned<S: TextSource>(old_tree: BlockTree, old_src: &S, edit: &Edit, new_src: &S) -> UpdateOutcome`; `pub fn incremental_update_instrumented_owned(old_tree: BlockTree, old_text: &str, edit: &Edit, new_text: &str) -> UpdateOutcome` (str convenience for the shell/tests); the `&`-entry `incremental_update_instrumented_src` retains its signature (delegates).

- [ ] **Step 1: Add `shift_in_place`, delete `shift_block`.** Replace `shift_block` (block_tree.rs:1269-1275) with an in-place mirror reusing `shift_range` (:1277-1279, unchanged):

```rust
/// Shift every span in `b`'s subtree by `delta`, IN PLACE (no allocation) — the in-place
/// twin of the former `shift_block`. Same arithmetic (`shift_range`), so the shifted subtree
/// is byte-identical to a `shift_block(&b, delta)` clone.
fn shift_in_place(b: &mut Block, delta: isize) {
    b.span = shift_range(&b.span, delta);
    for c in &mut b.children {
        shift_in_place(c, delta);
    }
}
```
(`shift_block`'s only caller was the splice at :975 — replaced below; grep to confirm before deleting.)

- [ ] **Step 2: Convert the function to owned + add the wrappers.** Rename the existing `pub fn incremental_update_instrumented_src<S: TextSource>(old_tree: &BlockTree, …)` (block_tree.rs:545) to `…_owned` and change the FIRST parameter from `&BlockTree` to `BlockTree` (by value). The body is UNCHANGED except the splice (Step 3) — the region computation still does `let tops = old_tree.top_level();` (borrows the owned tree; auto-ref works). Then add two thin entry points just above it:

```rust
/// `&`-entry (unchanged public signature; the oracle/fuzz/tests use this). Clones the old
/// tree and delegates to the owned path — the clone is cold (only tests take it; the shell
/// uses the owned entry directly).
pub fn incremental_update_instrumented_src<S: TextSource>(
    old_tree: &BlockTree, old_src: &S, edit: &Edit, new_src: &S,
) -> UpdateOutcome {
    incremental_update_instrumented_src_owned(old_tree.clone(), old_src, edit, new_src)
}

/// Owned str convenience (mirrors `incremental_update_instrumented`): the shell + the perf
/// test call this to hand the parser ownership of the old tree.
pub fn incremental_update_instrumented_owned(
    old_tree: BlockTree, old_text: &str, edit: &Edit, new_text: &str,
) -> UpdateOutcome {
    incremental_update_instrumented_src_owned(old_tree, &old_text, edit, &new_text)
}
```
(All existing `&`-entries — `incremental_update` :510, `incremental_update_instrumented` :522, `incremental_update_rope` :537, `incremental_update_src` :547 — already funnel through `incremental_update_instrumented_src`, so they now route through the owned path unchanged.)

- [ ] **Step 3: Replace the splice (block_tree.rs:956-1003) with the in-place version.** The two `partition_point`s are computed HERE (at the splice site, from the FINAL `region_old_start`/`region_old_end` — after the created-container second trigger). All borrows of `old_tree` (`tops`, `slack_block`, …) end before the `let mut children = old_tree.root.children` move (NLL); if any linger, copy the needed scalar out first.

```rust
    // F4: edit old_tree's owned `children` Vec in place instead of rebuilding it. The two
    // partition points reproduce the current before/overlap/after classification EXACTLY
    // (tops sorted by start + non-overlapping): before = span.end <= region_old_start;
    // after = span.start >= region_old_end && span.end > region_old_end; the dropped middle
    // is everything else (incl. the zero-length boundary block when region_old_start <
    // region_old_end). Both predicates are monotone over the sorted blocks.
    let splice_lo = tops.partition_point(|b| b.span.end <= region_old_start);
    let splice_hi =
        tops.partition_point(|b| !(b.span.start >= region_old_end && b.span.end > region_old_end));
    let reparsed_len = reparsed.root.children.len();
    let before_count = splice_lo;
    let after_seam = splice_lo + reparsed_len;

    // Consume old_tree: before-blocks [0, splice_lo) stay put (moved, not cloned); the
    // overlapping middle [splice_lo, splice_hi) is dropped; the reparsed blocks are MOVED in;
    // after-blocks (now [after_seam..]) shift IN PLACE (no per-node allocation).
    let mut children = old_tree.root.children;
    children.splice(splice_lo..splice_hi, reparsed.root.children);
    for b in &mut children[after_seam..] {
        shift_in_place(b, delta);
    }

    // SEAM CONSISTENCY — unchanged (after-blocks already shifted → new coords). before_count
    // and after_seam reproduce the old seam indices exactly.
    let merge_at = |i: usize| {
        i > 0
            && i < children.len()
            && paragraph_absorbs_next(new_src, &children[i - 1], &children[i])
    };
    if merge_at(before_count) || merge_at(after_seam) {
        let tree = full_parse_src(new_src);
        return UpdateOutcome {
            tree,
            reason: WidenReason::NoOverlapFull,
            reparsed_bytes: new_src.len(),
        };
    }

    let root = Block { kind: BlockKind::Document, span: 0..new_src.len(), children };
    UpdateOutcome { tree: BlockTree { root }, reason, reparsed_bytes }
```

- [ ] **Step 4: Run the full oracle/fuzz suite — the correctness proof.**
Run: `cargo test -p wordcartel-core` (the `block_tree_oracle.rs` suite + the in-lib `check` tests + proptests all assert `incremental == full_parse`, now via the owned path) and `cargo clippy --workspace --all-targets`.
Expected: PASS/clean — byte-identical output. A classification/shift/seam drift fails an existing oracle assertion. If the borrow-vs-move doesn't compile, extract the lingering borrow's scalar before the move (do NOT change behavior).

- [ ] **Step 5: Commit.**
```bash
git add wordcartel-core/src/block_tree.rs
git commit -m "perf(block_tree): in-place consume-and-splice — kill the per-keystroke splice allocations (F4)"   # + trailers
```

---

### Task 2: Shell wiring + the perf-proof test

**Files:** Modify `wordcartel/src/editor.rs` (`Document::take_blocks`); `wordcartel/src/derive.rs` (the incremental branch); `wordcartel-core/src/block_tree.rs` (the `#[cfg(test)]` perf test).

- [ ] **Step 1: Add `Document::take_blocks`** (editor.rs, in the `impl Document` block beside `set_blocks` :90):

```rust
    /// Take the derived block tree out by value, leaving a valid empty placeholder.
    /// TRANSIENT CONTRACT: the caller MUST write a real tree back (`set_blocks`, or the
    /// reconcile fallback's `apply_parse_result`) on EVERY path — until then the document
    /// holds an empty tree behind a stale `blocks_generation`. Does NOT bump the generation
    /// (only `set_blocks` does). Used by the incremental parse path to hand the parser
    /// ownership of the old tree (F4 — no clone).
    pub(crate) fn take_blocks(&mut self) -> wordcartel_core::block_tree::BlockTree {
        std::mem::replace(
            &mut self.blocks,
            wordcartel_core::block_tree::empty_tree(self.buffer.len()),
        )
    }
```
(Confirm the buffer field is `self.buffer` and `TextBuffer::len()` returns byte length — plan-confirm.)

- [ ] **Step 2: Route `derive.rs`'s incremental branch through the owned path.** In the incremental branch (the `if let (Some(old_rope), Some(edit)) = …` arm, ~derive.rs:127-148), replace the borrow-based call. Today it calls `incremental_update_instrumented_src(editor.active().document.blocks(), &old_rope, edit, &new_rope_ref)` inside `panicx::catch`. New: take the tree out FIRST, then move it into the closure:

```rust
                let new_rope_ref = &new_rope;
                let old_tree = editor.active_mut().document.take_blocks();
                match crate::panicx::catch(move || {
                    block_tree::incremental_update_instrumented_src_owned(
                        old_tree, &old_rope, edit, &new_rope_ref,
                    )
                }) {
                    Ok(outcome) => {
                        let stale = matches!(
                            outcome.reason,
                            block_tree::WidenReason::Local
                                | block_tree::WidenReason::WidenToEnd
                                | block_tree::WidenReason::BoundedStale
                        );
                        (outcome.tree, stale)
                    }
                    Err(msg) => (apply_parse_result(editor, new_len, Err(msg)), true),
                }
```
Panic-safety is preserved: `take_blocks` pulls a tree that is replaced on EVERY path — `set_blocks(new_blocks)` on `Ok` (the existing line after the `match`), or `apply_parse_result(…, Err)` (which returns the degraded empty tree, then flows into `set_blocks`) on a parse panic. The `move` closure captures `old_tree` (moved) + the `&`-refs; `panicx::catch` wraps it in `AssertUnwindSafe` (panicx.rs:31), so it compiles. Confirm `take_blocks`'s `&mut` borrow ends at the `take_blocks()` call (it returns an owned value) so it doesn't conflict with the later `editor` uses.

- [ ] **Step 3: Add the perf-proof test** in `block_tree.rs`'s `#[cfg(test)] mod tests`. It calls the OWNED entry directly (the `&`-wrappers clone → pointers would always differ) on a `Local` edit, and pins BOTH halves by `children.as_ptr()`:

```rust
    #[test]
    fn f4_splice_moves_before_and_shifts_after_without_reallocating() {
        // [list] [paras...] [list] — the edited middle paragraph is flanked by PARAGRAPHS
        // (not the lists), so its slack/upstream neighbors are paragraphs and the edit stays
        // Local (Codex: adjacent lists would trigger the container-merge widen). The first
        // list is a BEFORE block and the last list is an AFTER block, both with non-empty
        // children.
        let text = "- a\n- b\n\nlead\n\nmiddle\n\ntail one\n\ntail two\n\n- x\n- y\n";
        let old = full_parse(text);
        let before_idx = 0usize;
        let after_idx = old.root.children.len() - 1;
        assert!(!old.root.children[before_idx].children.is_empty());
        assert!(!old.root.children[after_idx].children.is_empty());
        let before_ptr = old.root.children[before_idx].children.as_ptr();
        let after_ptr = old.root.children[after_idx].children.as_ptr();
        let after_span0 = old.root.children[after_idx].span.clone();

        // Insert one char inside "middle" (a Local edit; region is the middle paragraph).
        let mid = text.find("middle").unwrap() + 3; // inside the word
        let (new_text, edit) = apply_edit(text, mid..mid, "X");
        let delta = 1isize;

        let outcome = incremental_update_instrumented_owned(old, text, &edit, &new_text);
        assert_eq!(outcome.reason, WidenReason::Local, "must stay on the splice path");
        // Byte-identical to a full parse (the free correctness net, restated locally):
        assert_eq!(outcome.tree, full_parse(&new_text));

        let t = &outcome.tree;
        let new_after_idx = t.root.children.len() - 1;
        // BEFORE block: moved, not deep-cloned → same inner children buffer pointer.
        assert_eq!(t.root.children[before_idx].children.as_ptr(), before_ptr,
            "before-block was deep-cloned instead of moved");
        // AFTER block: shifted IN PLACE, not shift_block-cloned → same pointer + shifted span.
        assert_eq!(t.root.children[new_after_idx].children.as_ptr(), after_ptr,
            "after-block was cloned instead of shifted in place");
        assert_eq!(t.root.children[new_after_idx].span,
            (after_span0.start + delta as usize)..(after_span0.end + delta as usize),
            "after-block span not shifted by delta");
    }
```
(Plan-confirm: verify the fixture's top level is `[List, Para, Para, Para, Para, List]` (first + last are Lists with non-empty children) and that inserting inside "middle" classifies `Local` (Codex-corrected fixture) — the top-level block COUNT must be unchanged by the char-insert so `before_idx=0` and `new_after_idx=last` still index the two lists. Adjust the fixture/edit position until `reason == Local`, `== full_parse`, and the two pointer asserts hold; the assertions are the contract and must NOT be weakened. `apply_edit`/`full_parse` are the existing in-lib test helpers.)

- [ ] **Step 4: Run + gates + commit.**
Run: `cargo test -p wordcartel-core -p wordcartel` (green — the oracle net + the perf test + the shell suite incl. the e2e journeys) + `cargo clippy --workspace --all-targets` (clean).
```bash
git add -A
git commit -m "perf(derive): route incremental parse through the owned splice + pin zero-copy (F4)"   # + trailers
```

---

## Self-Review

**Spec coverage:** owned entry + `&`-delegation (T1 S2, spec A1) ✓; in-place splice — partition points from final bounds + `Vec::splice` + `shift_in_place` + seam order (T1 S1/S3) ✓; `shift_block`→`shift_in_place` byte-identical via `shift_range` (T1 S1) ✓; `take_blocks` via `mem::replace`+`empty_tree`, `pub(crate)`, transient-contract doc (T2 S1, spec Minor-4) ✓; `derive.rs` owned call + panic-safety (T2 S2) ✓; the two-half owned-entry `Local`-edit perf pin (T2 S3, spec I-1/I-2) ✓; oracle/fuzz as the free correctness net (T1 S4) ✓. Scope held (splice only).

**Placeholder scan:** the perf-test fixture (exact top-level shape + `Local` classification) and the `self.buffer.len()` field are flagged as plan-confirms with "adjust the construction, never weaken the assertion" — appropriate; every other step carries complete code.

**Type consistency:** `incremental_update_instrumented_src_owned(old_tree: BlockTree, old_src: &S, …)` (owned first param); `&`-wrapper keeps `&BlockTree` + delegates via `.clone()` (`BlockTree: Clone` :164); `incremental_update_instrumented_owned(BlockTree, &str, &Edit, &str)`; `shift_in_place(&mut Block, isize)` reuses `shift_range`; `take_blocks(&mut self) -> BlockTree` via `mem::replace` + `empty_tree(usize)`; `WidenReason::Local` in the perf assertion; `delta: isize` in scope.

**Ordering:** T1 (core, oracle-proven byte-identical) is independent + fully behavior-preserving; T2 needs T1's owned entry + adds the shell wiring and the perf pin. Each task ends green + clippy-clean.
