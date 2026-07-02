# F1: Bounded WidenToEnd Reparse — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cap the synchronous `WidenToEnd` reparse so a keystroke inside a *small* container whose widen would speculatively reparse to EOF (Case A) stays O(base local block) instead of O(document) — via a size-gated `WidenReason::BoundedStale` that defers the container-wide effect to the existing 150 ms reconcile.

**Architecture:** Core-only change to `wordcartel-core/src/block_tree.rs`'s `incremental_update_instrumented_src`, plus a 1-line `derive.rs` wiring and an oracle-contract carve-out. The widen decision predicts the base local region on COPIES (via two extracted helpers), classifies three ways (WidenToEnd if the extension ≤ cap; BoundedStale if the extension > cap but the base ≤ cap; WidenToEnd/today if the base itself > cap = Case B), and installs the copied bounds only for BoundedStale — leaving the `WidenToEnd` and `Local` paths byte-identical.

**Tech Stack:** Rust, `wordcartel-core` (`#![forbid(unsafe_code)]`) + `wordcartel` shell.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-02-wordcartel-f1-bounded-widen-reparse-design.md` (Codex ×6 + Fable5 folded).
- `cargo test -p wordcartel-core -p wordcartel` green; `cargo build`/`test --no-run` warning-free; **`cargo clippy --workspace --all-targets` clean (deny gate LIVE)**; NO `cargo fmt`; house style (em-dash `—`, doc-comment public items).
- `#![forbid(unsafe_code)]` in core unchanged.
- **Behavior-preserving for the `WidenToEnd`/`Local`/Case-B paths** — the existing block-tree oracle + proptest + fuzz suites stay green (small docs never reach the 1 MiB cap → `BoundedStale` is never emitted by them).
- Cap: `MAX_SYNC_WIDEN_BYTES = 1 << 20` (1 MiB) — a named tunable; validate the real per-MiB cost during implementation and lower (e.g. 512 KiB) if a bounded parse exceeds a frame (spec M6).
- Trailers on every commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: Core — the size-gated widen + behavioral tests

**Files:**
- Modify: `wordcartel-core/src/block_tree.rs` (the `WidenReason` enum ~:477; a new const; two extracted helpers; the widen decision ~:767-828; the shared straddle/trailing-gap ~:836-865; the second trigger ~:915; new `#[cfg(test)]` tests).
- Modify: `wordcartel/src/derive.rs:139-142` (the `maybe_stale` `matches!`).

**Interfaces produced:** `WidenReason::BoundedStale`; `pub const MAX_SYNC_WIDEN_BYTES: usize`; the outcome's `reason` may now be `BoundedStale` (reparsed the base local region only, `reparsed_bytes ≤ cap`, tree stale beyond the base).

- [ ] **Step 1: Add the `BoundedStale` variant.** In `WidenReason` (block_tree.rs:478-485), after `WidenToEnd`:

```rust
    /// A hard trigger forced reparsing to end-of-document.
    WidenToEnd,
    /// The widen EXTENSION to EOF would exceed `MAX_SYNC_WIDEN_BYTES` while the base
    /// local region is small (Case A). Reparsed only the base local region and left the
    /// container-wide effect (loose/tight, absorb-to-EOF) STALE; `derive` marks it
    /// `maybe_stale` and the debounced reconcile converges it to `full_parse` at rest.
    BoundedStale,
```

- [ ] **Step 2: Add the cap const** near the top of `block_tree.rs` (beside other module consts; place it just above `pub fn incremental_update`):

```rust
/// Synchronous-reparse ceiling for the widen path (F1). When a widen's speculative
/// extension to end-of-document would exceed this many bytes but the base local region
/// is smaller, the reparse is bounded to the base region and tagged `BoundedStale`
/// (the container-wide effect is deferred to the reconcile). A named tunable — validate
/// the real per-MiB `parse_region` cost and lower if a bounded parse exceeds a frame.
pub const MAX_SYNC_WIDEN_BYTES: usize = 1 << 20; // 1 MiB
```

- [ ] **Step 3: Extract two region-finalization helpers** (free functions, generic over `TextSource`), so the real path AND the copy-prediction share ONE implementation (spec M2 — no replication/drift). Add them just above `incremental_update_instrumented_src` (block_tree.rs ~:544). The bodies are the VERBATIM current logic moved out of the function:

```rust
/// The Local-path region extension: `+1` slack block, plus (for gap edits) the upstream
/// blank-delimited-group pull-back. Mutates the candidate region `[start, end)` in place.
/// Extracted from the `else` branch of the widen decision so the F1 copy-prediction can
/// apply the SAME rule to a copy.
fn apply_local_slack<S: TextSource>(
    tops: &[Block],
    old_src: &S,
    start: &mut usize,
    end: &mut usize,
    slack_pos: Option<usize>,
    have_overlap: bool,
) {
    if let Some(slack_idx) = slack_pos {
        if slack_idx + 1 < tops.len() {
            *end = tops[slack_idx + 1].span.start;
        } else {
            *end = old_src.len();
        }
    }
    if !have_overlap {
        if let Some(b) = tops.iter().rev().find(|b| b.span.end <= *start) {
            *start = blank_delimited_group_start(old_src, old_src.line_start(b.span.start));
        }
    }
}

/// Straddle repair + trailing-gap coverage on `[start, end)`. Shared by every path;
/// idempotent (a region already repaired grows no further). Extracted from the code
/// after the widen decision.
fn repair_region<S: TextSource>(tops: &[Block], old_src: &S, start: &mut usize, end: &mut usize) {
    loop {
        let mut grew = false;
        for b in tops.iter() {
            if b.span.start < *start && b.span.end > *start {
                *start = old_src.line_start(b.span.start);
                grew = true;
            }
            if b.span.start < *end && b.span.end > *end {
                *end = old_src.line_end(b.span.end);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    let has_after_block = tops.iter().any(|b| b.span.start >= *end && b.span.end > *end);
    if !has_after_block && *end < old_src.len() {
        *end = old_src.len();
    }
}
```

- [ ] **Step 4: Route the existing paths through the helpers** (behavior-preserving). In `incremental_update_instrumented_src`:
  - Replace the `else` branch body (block_tree.rs:775-828 — the big slack/pullback comment block and its code) with a call, keeping the comment as a one-line pointer:
    ```rust
    } else {
        // +1 slack block + (gap-edit) upstream pull-back — see `apply_local_slack`.
        apply_local_slack(tops, old_src, &mut region_old_start, &mut region_old_end, slack_pos, have_overlap);
        reason = WidenReason::Local;
    }
    ```
  - Replace the shared straddle-repair loop (block_tree.rs:830-853) AND the trailing-gap block (block_tree.rs:855-865) with a single call (keep the `debug_assert!`s that follow at :870-878):
    ```rust
    // Straddle repair + trailing-gap coverage — see `repair_region`.
    repair_region(tops, old_src, &mut region_old_start, &mut region_old_end);
    ```

- [ ] **Step 5: Run the suite — confirm the extraction is behavior-preserving.**
Run: `cargo test -p wordcartel-core`
Expected: PASS (identical behavior — the helpers contain the same logic; no path emits `BoundedStale` yet).

- [ ] **Step 6: Add the three-way gate** in the `if widen` branch. Replace `if widen { region_old_end = old_src.len(); reason = WidenReason::WidenToEnd; }` (block_tree.rs:772-774) with:

```rust
    if widen {
        // F1: predict the base local region on COPIES (what the Local path would produce),
        // then bound the widen only when the EXTENSION is the expense (Case A). The copies
        // apply the same slack/pull-back + straddle/trailing-gap the Local path would; fm_floor
        // is deliberately NOT applied to the copies (conservative — can only misclassify A→B).
        let mut base_start = region_old_start;
        let mut base_end = region_old_end;
        apply_local_slack(tops, old_src, &mut base_start, &mut base_end, slack_pos, have_overlap);
        repair_region(tops, old_src, &mut base_start, &mut base_end);
        // Size in NEW-text bytes — `reparsed_bytes` is `new_region.len()` AFTER `delta` (Codex):
        // an insertion can push the actual reparse over the cap even when the old base fits.
        let base_new_end = (base_end as isize + delta) as usize;
        let base_region_size = base_new_end.saturating_sub(base_start);
        let widen_span = new_src.len().saturating_sub(region_old_start);
        if widen_span <= MAX_SYNC_WIDEN_BYTES {
            // cheap extension → widen fully, exactly as today
            region_old_end = old_src.len();
            reason = WidenReason::WidenToEnd;
        } else if base_region_size <= MAX_SYNC_WIDEN_BYTES {
            // Case A: expensive extension, small base → install the copied base bounds, defer
            region_old_start = base_start;
            region_old_end = base_end;
            reason = WidenReason::BoundedStale;
        } else {
            // Case B: the base local region itself exceeds the cap → widen as today
            region_old_end = old_src.len();
            reason = WidenReason::WidenToEnd;
        }
    } else {
```

(The subsequent shared `repair_region` call from Step 4 runs on the installed BoundedStale bounds idempotently; `fm_floor` at :892 then clamps `region_old_start`, so the actual reparse is ≤ `base_region_size` ≤ cap.)

- [ ] **Step 7: Size-gate the second (created-container) widen — the SAME three-way logic (I1 + Codex Important-2).** The created-container growth (block_tree.rs:915-929) unconditionally extends to EOF when the reparse's tail is absorptive. Two problems: (a) a first-gate `BoundedStale` would get re-extended to EOF (~3×cap — I1); (b) a `Local` arrival whose edit CREATES an absorptive container reaching the boundary (e.g. typing `- ` at the top of a large doc) still widens to EOF synchronously — an unbounded freeze the spec's second-trigger gate is meant to bound. Note: a first-gate `WidenToEnd` never reaches here (`region_old_end == old_src.len()` already, per the :913 comment), so this block only sees `Local`/`BoundedStale` arrivals. Replace the `if tail_absorptive { ... }` body (block_tree.rs:922-928) with the size gate — keeping the already-parsed `reparsed`/`new_region` (base) when bounding, so NO second parse happens in Case A:

```rust
        if tail_absorptive {
            // F1: gate the second widen the same three-way way as the first.
            let widen_span = new_src.len().saturating_sub(region_new_start);
            let base_new_size = region_new_end.saturating_sub(region_new_start);
            if widen_span <= MAX_SYNC_WIDEN_BYTES || base_new_size > MAX_SYNC_WIDEN_BYTES {
                // cheap extension, or Case B (base already > cap) → extend to EOF, as today
                region_old_end = old_src.len();
                region_new_end = (region_old_end as isize + delta) as usize;
                new_region = new_src.slice(region_new_start..region_new_end);
                reparsed = parse_region(&new_region.as_ref(), 0..new_region.len(), region_new_start);
                reason = WidenReason::WidenToEnd;
            } else {
                // Case A: keep the already-parsed base region (no second parse), defer.
                reason = WidenReason::BoundedStale;
            }
        }
```
(For a first-gate `BoundedStale` arriving here, `base_new_size ≤ cap` so the `else` keeps it `BoundedStale` with no re-parse — subsuming the I1 skip; for a `Local` arrival with a big absorptive tail, `widen_span > cap` and `base_new_size ≤ cap` → bound it. `reparsed_bytes = new_region.len()` stays the already-parsed base ≤ cap.)

- [ ] **Step 8: Wire `derive.rs`.** In `wordcartel/src/derive.rs:139-142`, add `BoundedStale` to the stale `matches!`:
```rust
                        let stale = matches!(
                            outcome.reason,
                            block_tree::WidenReason::Local
                                | block_tree::WidenReason::WidenToEnd
                                | block_tree::WidenReason::BoundedStale
                        );
```

- [ ] **Step 9: Add the behavioral tests** in `block_tree.rs`'s `#[cfg(test)] mod tests` (they use the in-module `incremental_update_instrumented` + a local validity walk). Add a shared helper and five tests:

```rust
    /// Lightweight, linear structural-validity walk (M1: the F3 helper is private/quadratic).
    /// Root spans the whole doc; child spans are ordered + non-overlapping at every level
    /// (gaps between blocks allowed — children do not tile).
    fn assert_valid_tree(t: &BlockTree, new_len: usize) {
        assert_eq!(t.root.span, 0..new_len, "root must span [0, new_len)");
        fn walk(children: &[Block]) {
            let mut prev_end = 0usize;
            for c in children {
                assert!(c.span.end >= c.span.start, "span well-formed: {:?}", c.span);
                assert!(c.span.start >= prev_end, "children ordered/non-overlapping: {:?}", c.span);
                prev_end = c.span.end;
                walk(&c.children);
            }
        }
        walk(&t.root.children);
    }

    #[test]
    fn f1_case_a_small_container_widen_is_bounded() {
        // ~1.56 MiB of paragraphs; the enclosing block of an edit near the top is small,
        // but inserting an opening fence fires `needs_widen_to_end` → the extension reaches
        // EOF (> 1 MiB) → BoundedStale (bounded to the small base region).
        let doc = "para\n\n".repeat(260_000);
        assert!(doc.len() > MAX_SYNC_WIDEN_BYTES, "doc must exceed the cap");
        let (new_text, edit) = apply_edit(&doc, 0..0, "```\n");
        let old_tree = full_parse(&doc);
        let outcome = incremental_update_instrumented(&old_tree, &doc, &edit, &new_text);
        assert_eq!(outcome.reason, WidenReason::BoundedStale, "expected a bounded reparse");
        assert!(outcome.reparsed_bytes <= MAX_SYNC_WIDEN_BYTES,
            "reparsed {} bytes, cap {}", outcome.reparsed_bytes, MAX_SYNC_WIDEN_BYTES);
        assert_valid_tree(&outcome.tree, new_text.len());
        let full = full_parse(&new_text);
        assert_ne!(outcome.tree, full, "BoundedStale is deliberately stale vs full_parse");
        // full_parse is the convergence target the reconcile installs.
        assert_eq!(full.root.span, 0..new_text.len());
    }

    #[test]
    fn f1_small_extension_still_widens_fully() {
        // Small doc: the widen extension is ≤ cap → WidenToEnd, byte-identical to today.
        let doc = "- a\n- b\n\npara\n";
        let (new_text, edit) = apply_edit(doc, 0..0, "```\n");
        let old_tree = full_parse(doc);
        let outcome = incremental_update_instrumented(&old_tree, doc, &edit, &new_text);
        assert_ne!(outcome.reason, WidenReason::BoundedStale, "small docs must not bound");
        assert_eq!(outcome.tree, full_parse(&new_text), "≤cap path stays == full_parse");
    }

    #[test]
    fn f1_case_b_single_huge_container_falls_through() {
        // A single doc-spanning list (> 1 MiB): the BASE local region is already the whole
        // container → base > cap → falls through to WidenToEnd (never BoundedStale).
        let doc = "- item\n".repeat(200_000); // ~1.4 MiB, one list
        assert!(doc.len() > MAX_SYNC_WIDEN_BYTES);
        let (new_text, edit) = apply_edit(&doc, 7..7, "- x\n"); // edit inside the list near the top
        let old_tree = full_parse(&doc);
        let outcome = incremental_update_instrumented(&old_tree, &doc, &edit, &new_text);
        assert_ne!(outcome.reason, WidenReason::BoundedStale,
            "a single >cap container is Case B — not bounded");
    }

    #[test]
    fn f1_created_container_local_is_bounded() {
        // Typing "- " at the top of a big paragraph doc CREATES an absorptive list — the OLD
        // tree has no container there, so the FIRST gate is Local; the created-container
        // SECOND trigger would extend to EOF. F1's second-trigger gate must bound it.
        let doc = "para\n\n".repeat(260_000);
        assert!(doc.len() > MAX_SYNC_WIDEN_BYTES);
        let (new_text, edit) = apply_edit(&doc, 0..0, "- ");
        let outcome = incremental_update_instrumented(&full_parse(&doc), &doc, &edit, &new_text);
        assert_eq!(outcome.reason, WidenReason::BoundedStale,
            "a created-container Local edit on a big doc must bound at the second trigger");
        assert!(outcome.reparsed_bytes <= MAX_SYNC_WIDEN_BYTES);
        assert_valid_tree(&outcome.tree, new_text.len());
    }

    #[test]
    fn f1_successive_bounded_stale_edits_no_reset() {
        // Production feeds a BoundedStale tree into the NEXT incremental_update WITHOUT a
        // reset (unlike the oracle). Two bounded edits in a row must stay panic-free + valid.
        let doc = "para\n\n".repeat(260_000);
        let (t1_text, e1) = apply_edit(&doc, 0..0, "```\n");
        let o1 = incremental_update_instrumented(&full_parse(&doc), &doc, &e1, &t1_text);
        assert_eq!(o1.reason, WidenReason::BoundedStale);
        // Second bounded edit, fed the STALE tree o1.tree (no reset):
        let (t2_text, e2) = apply_edit(&t1_text, 0..0, "```\n");
        let o2 = incremental_update_instrumented(&o1.tree, &t1_text, &e2, &t2_text);
        assert_valid_tree(&o2.tree, t2_text.len()); // no panic + valid
        // Convergence target exists (what reconcile computes):
        assert_eq!(full_parse(&t2_text).root.span, 0..t2_text.len());
    }
```

- [ ] **Step 10: Run + gates + commit.**
Run: `cargo test -p wordcartel-core -p wordcartel` (green) and `cargo clippy --workspace --all-targets` (clean).
Note: the five new tests assert `reason`/validity directly (not `== full_parse`), so they pass WITHOUT the Task-2 carve-out. If `f1_case_a_*` or `f1_case_b_*` does not produce the expected `reason`, adjust the doc construction until the classification holds (the assertion is the spec's contract), then re-run.
```bash
git add -A
git commit -m "feat(block_tree): bound the synchronous WidenToEnd reparse (F1 Case A)"   # + trailers
```

---

### Task 2: Oracle-contract carve-out for `BoundedStale`

**Files:**
- Modify: `wordcartel-core/tests/block_tree_oracle.rs` (the `assert_all_paths_agree!` macro :24-45, `assert_chain_paths_agree!` :53-80, `check` :88-99).
- Modify: `wordcartel-core/src/block_tree.rs` (`incremental_equals_full` :1281-1284 — the cargo-fuzz F2 oracle; the in-module `check` :1325-1332).

**Interfaces consumed:** `WidenReason::BoundedStale`, `incremental_update_instrumented`, `incremental_update_instrumented_src` (public, generic over str+rope), `UpdateOutcome`.

**Why:** `incremental == full` is asserted unconditionally at these sites; a `BoundedStale` tree is deliberately `!= full` by design. The generators/fixtures never reach the cap TODAY (so all suites already pass), but the carve-out is contract-correctness and prevents a **false-positive trap** if a future proptest/fuzz corpus reaches ≥ 1 MiB (spec I4). This task adds NO new emission — purely guards the equality assertions.

- [ ] **Step 1: Carve out the `check` helper** (block_tree_oracle.rs:88-99 — already instrumented). Guard the equality:
```rust
    let full = full_parse(&new_text);
    if outcome.reason != wordcartel_core::block_tree::WidenReason::BoundedStale {
        assert_eq!(
            outcome.tree, full,
            "\nINCREMENTAL != FULL\nold_text={old_text:?}\nnew_text={new_text:?}\nreason={:?}\nincremental={:#?}\nfull={:#?}",
            outcome.reason, outcome.tree, full
        );
    }
    outcome
```

- [ ] **Step 2: Convert `assert_all_paths_agree!`** (block_tree_oracle.rs:24-45) to read the reason on BOTH paths, assert the reasons agree (M3), and skip the two `== full` assertions for `BoundedStale` (keep `rope == str`):
```rust
macro_rules! assert_all_paths_agree {
    ($old:expr, $edit:expr, $new:expr) => {{
        let old: &str = $old;
        let edit = $edit;
        let new: &str = $new;
        let ot = full_parse(old);
        let full = full_parse(new);
        let str_out = wordcartel_core::block_tree::incremental_update_instrumented(&ot, old, edit, new);
        // TextSource is impl'd for `&Rope`, so S = &Rope and the generic's `&S` needs `&&Rope`
        // (mirrors derive.rs:129-132). Bind the ropes to locals, then pass `&&local`.
        let old_rope = Rope::from_str(old);
        let new_rope = Rope::from_str(new);
        let rope_out = wordcartel_core::block_tree::incremental_update_instrumented_src(
            &ot, &&old_rope, edit, &&new_rope,
        );
        prop_assert_eq!(str_out.reason, rope_out.reason,
            "\nstr reason != rope reason\nold={:?}\nnew={:?}", old, new);
        if str_out.reason != wordcartel_core::block_tree::WidenReason::BoundedStale {
            prop_assert_eq!(&str_out.tree, &full,
                "\nstr path != full_parse\nold={:?}\nnew={:?}", old, new);
            prop_assert_eq!(&rope_out.tree, &full,
                "\nrope path != full_parse\nold={:?}\nnew={:?}", old, new);
        }
        prop_assert_eq!(&rope_out.tree, &str_out.tree,
            "\nrope path != str path\nold={:?}\nnew={:?}", old, new);
    }};
}
```

- [ ] **Step 3: Convert `assert_chain_paths_agree!`** (block_tree_oracle.rs:53-80) to read the reason, assert reasons agree, skip `== full` for `BoundedStale`, and RESET the carried trees to `full_parse` on `BoundedStale` (prevents the oracle's `== full` chain from cascading — production does not reset, which Task 1's `f1_successive_*` test covers):
```rust
        for (edit, new_text) in edits {
            let str_out = wordcartel_core::block_tree::incremental_update_instrumented(&str_tree, &text, edit, new_text);
            // S = &Rope → pass `&&local` (see the single-edit macro).
            let text_rope = Rope::from_str(&text);
            let new_rope = Rope::from_str(new_text);
            let rope_out = wordcartel_core::block_tree::incremental_update_instrumented_src(
                &rope_tree, &&text_rope, edit, &&new_rope,
            );
            let full = full_parse(new_text);
            prop_assert_eq!(str_out.reason, rope_out.reason,
                "\nchain: str reason != rope reason\nbefore={:?}\nafter={:?}", text, new_text);
            if str_out.reason != wordcartel_core::block_tree::WidenReason::BoundedStale {
                prop_assert_eq!(&str_out.tree, &full,
                    "\nchain: str path != full_parse\nbefore={:?}\nafter={:?}", text, new_text);
                prop_assert_eq!(&rope_out.tree, &full,
                    "\nchain: rope path != full_parse\nbefore={:?}\nafter={:?}", text, new_text);
            }
            prop_assert_eq!(&rope_out.tree, &str_out.tree,
                "\nchain: rope path != str path\nbefore={:?}\nafter={:?}", text, new_text);
            // On BoundedStale, reset to full_parse so the NEXT step's `== full` stays meaningful
            // (a stale carried tree would make every subsequent comparison spurious).
            if str_out.reason == wordcartel_core::block_tree::WidenReason::BoundedStale {
                str_tree = full_parse(new_text);
                rope_tree = full_parse_rope(&Rope::from_str(new_text));
            } else {
                str_tree = str_out.tree;
                rope_tree = rope_out.tree;
            }
            text = new_text.clone();
        }
```

- [ ] **Step 4: Carve out the cargo-fuzz F2 oracle** `incremental_equals_full` (block_tree.rs:1281-1284) — the highest-value site (a future fuzz run with `max_len ≥ cap` would emit `BoundedStale` and report spurious failures indistinguishable from real F2 divergences):
```rust
#[cfg(any(test, fuzzing))]
pub fn incremental_equals_full(old: &str, range: std::ops::Range<usize>, repl: &str) -> bool {
    let (new, edit) = apply_edit(old, range, repl);
    let outcome = incremental_update_instrumented(&full_parse(old), old, &edit, &new);
    // BoundedStale is deliberately != full_parse (converged later by reconcile); it is NOT a
    // divergence bug, so the oracle treats it as a pass.
    outcome.reason == WidenReason::BoundedStale || outcome.tree == full_parse(&new)
}
```

- [ ] **Step 5: Carve out the in-module `check`** (block_tree.rs:1325-1332), same guard as Step 1:
```rust
        let full = full_parse(&new_text);
        if outcome.reason != WidenReason::BoundedStale {
            assert_eq!(
                outcome.tree, full,
                // ...keep the existing message...
            );
        }
        outcome
```

- [ ] **Step 6: Document the exempt small-fixed-doc sites** (spec I4 — no conversion needed; they can never reach the cap). Add a one-line comment at `assert_all_paths_agree_det` (block_tree_oracle.rs:803) and at the delete-to-empty/single-char tests (:704/:723) noting they use small fixed docs that never emit `BoundedStale`, so their unconditional `== full` stays valid.

- [ ] **Step 7: Run + gates + commit.**
Run: `cargo test -p wordcartel-core -p wordcartel` (green); `cargo clippy --workspace --all-targets` (clean); `cargo build --manifest-path wordcartel-core/fuzz/Cargo.toml` if the fuzz crate builds in this environment (else note it's cfg(fuzzing) and unbuilt here).
```bash
git add -A
git commit -m "test(block_tree): carve BoundedStale out of the incremental==full oracles (F1)"   # + trailers
```

---

## Self-Review

**Spec coverage:** `BoundedStale` variant + `MAX_SYNC_WIDEN_BYTES` (T1 S1-2) ✓; predict-on-copies three-way gate via shared helpers (T1 S3-6, spec M2, new-length sizing per Codex) ✓; unified second-trigger three-way gate covering BOTH the I1 first-gate-BoundedStale skip AND the Local-arrival created-container bound (T1 S7 + the `f1_created_container_local_is_bounded` test) ✓; `derive` wiring (T1 S8) ✓; I2 seam-guard → `NoOverlapFull` is automatic (block_tree.rs:975-979, no code change — confirmed in Task 1 review) ✓; oracle carve-out incl. the fuzz F2 oracle (T2, spec I4) ✓; str==rope reason (T2 S2-3, M3) ✓; Case-A/small-ext/Case-B/successive-BoundedStale tests (T1 S9, spec I3) ✓; non-quadratic local validity helper (T1 S9, M1) ✓; documented exemptions (T2 S6) ✓. Reconcile unchanged ✓. M4 (parse-panic enlarged stale) + M6 (cap tunable) are documented tradeoffs, no code.

**Placeholder scan:** none — every code step carries complete code. The one runtime-tunable is the doc construction in `f1_case_a_*`/`f1_case_b_*` (T1 S10 notes: adjust the construction if the classification differs — the `reason` assertion is the contract).

**Type consistency:** `apply_local_slack`/`repair_region` take `tops: &[Block]` (matches `top_level() -> &[Block]`) + `&S: TextSource`; `incremental_update_instrumented_src` is generic over str+rope (no separate rope variant); `WidenReason` is `Copy` (assert on `reason` by value); `Block`/`BlockTree` fields are `pub` (the validity walk recurses from the external oracle crate). The gate uses `slack_pos` (block_tree.rs:727) + `have_overlap` (:572), both in scope at :767.

**Ordering:** Task 1 is behavior-preserving for existing suites (its Step 5 checkpoint proves the extraction) and its new tests assert `reason` directly (independent of Task 2). Task 2 only guards equality assertions (no new emission). Either task compiles and tests independently; Task 1 first (it defines the variant Task 2 references).
