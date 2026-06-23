# Wordcartel block_tree — Implementation Plan (Effort 3, Plan 3a)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the `block_tree` module to `wordcartel-core`: incremental Markdown block-structure parsing — a `full_parse` (the oracle) and an `incremental_update` that reparses only a safe enclosing region on edit, gated by a strict **incremental == full** property test. Ported from the validated block-tree spike.

**Architecture:** Maintain a `BlockTree` (document → container blocks → leaf blocks), each block carrying a source byte `span` and a `kind`. On an edit, compute a safe reparse region (snap to lines, pull back over the current line-group / preceding containers, never landing inside a code block), reparse that slice, and splice it back — with **full-reparse fallback** for HTML and **widen-to-end** for link-ref-defs / fence-marker touches / container overlap. This honors the "build incremental block-tree now" decision at **region-reparse granularity** (the spike-validated v1); fine-grained sub-block tracking is deferred.

**Tech Stack:** Rust 2021; existing `pulldown-cmark 0.13`; `proptest`. Headless, pure. Builds on the merged edit kernel + render core.

## Global Constraints

- Crate `wordcartel-core`; `#![forbid(unsafe_code)]`; pure/headless (no io/threads/terminal).
- Canonical position = **byte offset** (`usize`). Spans are **absolute** byte ranges into the document text (see Reuse Posture for why v1 keeps absolute, not relative).
- Parse with `pulldown-cmark 0.13` `Parser::new_ext(...).into_offset_iter()`; GFM tables + strikethrough enabled (match the spike's options).
- The code is **ported from the validated spike** at `~/projects/wordcartel-blocktree-spike/src/lib.rs` and its oracle `~/projects/wordcartel-blocktree-spike/tests/oracle.rs` (the implementer READS those files). Port the algorithm **verbatim** except for module placement/imports; do not "improve" the invalidation logic — it was validated across ~800,000 oracle cases and 8 hand-fixed context hazards.
- The **incremental == full oracle** (Task 3) is the merge gate: it MUST cover every container construct (paragraph, ATX & setext headings, fenced code spanning blanks, blockquote incl. lazy continuation, lists incl. multi-paragraph/nested items, thematic break, HTML block, link-reference-definition).
- TDD; pristine output; commit `proptest-regressions/` seeds.

---

## Reuse Posture

This module ports our OWN spike — the validated reference for the project's riskiest algorithm. **v1 keeps the spike's absolute spans** (with its O(n) span-shift on edit) rather than the spike's *recommended* relative-span optimization, because: (a) the absolute-span code is the version validated @~800k oracle cases — porting it verbatim preserves that correctness in the highest-bug-surface module; (b) the O(n) shift measured ~1 ms @ 5 MB, within the §3.9 budget; (c) relative spans are new, unvalidated code and belong in a later, separately-oracle-gated optimization. **Relative-span O(1) shift is explicitly deferred.**

---

## File Structure

- `wordcartel-core/src/lib.rs` — declare `pub mod block_tree;`.
- `wordcartel-core/src/block_tree.rs` — the module: types, `full_parse`, `incremental_update`, region/splice logic.
- `wordcartel-core/tests/block_tree_oracle.rs` — the incremental==full oracle: hazard regressions + proptest.

---

### Task 0: Scaffold block_tree module

**Files:** Modify `src/lib.rs`; create `src/block_tree.rs` (stub).

- [ ] **Step 1:** In `src/lib.rs`, add `pub mod block_tree;` with the other module decls.
- [ ] **Step 2:** Create `src/block_tree.rs` containing only `// filled in by later tasks`.
- [ ] **Step 3:** Run `cargo build --manifest-path wordcartel-core/Cargo.toml` → clean.
- [ ] **Step 4:** Commit: `chore(core): scaffold block_tree module`

---

### Task 1: Block types + full_parse (the oracle)

**Files:** Modify `src/block_tree.rs`.

**Port source:** `~/projects/wordcartel-blocktree-spike/src/lib.rs` — **READ it** — port **verbatim** (adjust only imports): `BlockKind` (enum, ~line 15), `Block { kind, span: Range<usize>, children: Vec<Block> }` (~34), `BlockTree { root: Block }` + `top_level()` (~43-55), and `full_parse(text: &str) -> BlockTree` (~101-153, the pulldown event-walk with a container stack; `Event::Rule` → `ThematicBreak`). Keep the GFM `Options` exactly as the spike sets them.

**Interfaces — Produces:** `pub enum BlockKind {...}`, `pub struct Block {...}`, `pub struct BlockTree {...}` (+ `top_level(&self) -> &[Block]`), `pub fn full_parse(text: &str) -> BlockTree`. All `#[derive(Clone, Debug, PartialEq, Eq)]` as the spike has them.

- [ ] **Step 1: Write failing unit tests** in `src/block_tree.rs` (`#[cfg(test)] mod tests`) — port a handful of the spike's deterministic hazard checks as direct `full_parse` assertions, e.g.:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn kinds(t: &BlockTree) -> Vec<BlockKind> {
        t.top_level().iter().map(|b| b.kind).collect()
    }
    #[test]
    fn parses_heading_and_paragraph() {
        let t = full_parse("# Title\n\nbody text\n");
        assert_eq!(kinds(&t), vec![BlockKind::Heading, BlockKind::Paragraph]);
    }
    #[test]
    fn fenced_code_spans_blank_lines_as_one_block() {
        let t = full_parse("```\na\n\nb\n```\n");
        assert_eq!(kinds(&t), vec![BlockKind::FencedCode]); // the blank line is INSIDE the fence
    }
    #[test]
    fn blockquote_is_a_container() {
        let t = full_parse("> quoted\n");
        assert_eq!(t.top_level()[0].kind, BlockKind::BlockQuote);
        assert!(!t.top_level()[0].children.is_empty()); // contains a paragraph
    }
}
```
(Adjust the exact `BlockKind` variant names to match the spike's enum.)
- [ ] **Step 2:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml block_tree` → FAIL.
- [ ] **Step 3:** Port the types + `full_parse` from the spike (verbatim, imports adjusted).
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(core): block_tree types + full_parse (ported from spike)`

---

### Task 2: incremental_update (the validated invalidation algorithm)

**Files:** Modify `src/block_tree.rs`.

**Port source:** the spike `src/lib.rs` — port **verbatim** (imports adjusted): `Edit { range: Range<usize>, new_len: usize }` (+ `delta()`), `WidenReason` enum, `UpdateOutcome`, `incremental_update(old_tree, old_text, edit, new_text) -> BlockTree`, `incremental_update_instrumented(...) -> UpdateOutcome`, the `apply_edit` test helper, and ALL the private region-computation + splice helpers they call (lines ~156-560). **Do not alter the invalidation logic** — every clause (line-group pull-back, code-block clamp, full-reparse-on-HTML, widen-to-end on ref-def/fence-line/container-overlap, invariant-repair loop, span-shift, splice) was validated by the oracle. Reproduce them exactly.

**Interfaces — Produces:** `pub struct Edit {...}`, `pub enum WidenReason { Local, WidenToEnd, NoOverlapFull }`, `pub struct UpdateOutcome { tree, reason, reparsed_bytes }`, `pub fn incremental_update(...)`, `pub fn incremental_update_instrumented(...)`, `pub fn apply_edit(old_text, range, replacement) -> (String, Edit)`.

- [ ] **Step 1: Write failing tests** — port 4–5 of the spike's deterministic hazard regressions from `tests/oracle.rs` as in-module tests that compare `incremental_update` to `full_parse(new_text)` for specific edits (e.g. typing in a paragraph → Local; toggling a fence marker → tree still equals full; editing near HTML → still equals full). Each asserts `incremental_update(&old_tree, &old, &edit, &new) == full_parse(&new)`.
- [ ] **Step 2:** Run `cargo test ... block_tree` → FAIL (no `incremental_update`).
- [ ] **Step 3:** Port `Edit`/`WidenReason`/`UpdateOutcome`/`incremental_update`/`incremental_update_instrumented`/`apply_edit` + all private helpers verbatim from the spike.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(core): block_tree incremental_update (ported from spike)`

---

### Task 3: The incremental == full oracle (merge gate)

**Files:** Create `wordcartel-core/tests/block_tree_oracle.rs`.

**Port source:** `~/projects/wordcartel-blocktree-spike/tests/oracle.rs` — **READ it** — port its 18 named hazard regression tests AND the proptest oracle into an integration test file against the crate's public `block_tree` API. The proptest: generate random markdown from a construct-mixing alphabet (paragraphs, ATX/setext headings, fenced code with internal blanks, blockquotes with lazy continuation, lists with multi-paragraph/nested items, thematic breaks, HTML blocks, link-ref-defs) + a random edit, and assert `incremental_update(&full_parse(old), old, &edit, new) == full_parse(new)`. Use ≥512 cases.

- [ ] **Step 1:** Create `tests/block_tree_oracle.rs` porting the spike's hazard regressions + the proptest oracle (adapt paths to `wordcartel_core::block_tree::*`).
- [ ] **Step 2:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml --test block_tree_oracle`. It MUST pass (the algorithm was validated @~800k cases). **If any case FAILS, the port diverged from the spike** — proptest prints a counterexample; fix the ported `block_tree.rs` to match the spike (do NOT weaken the oracle). If you cannot resolve it, report DONE_WITH_CONCERNS with the counterexample.
- [ ] **Step 3:** Run the full suite (no regressions); commit (include `proptest-regressions/`): `test(core): block_tree incremental==full oracle (18 hazards + proptest)`

---

## Self-Review (completed during planning)

- **Spec coverage:** §9.2 `block_tree` incremental invalidation (the spike-validated v1) → Tasks 1–3; the incremental==full oracle (§11.2) → Task 3. Block-role *rendering* and wiring roles into md_parse/layout → **Plan 3b** (out of scope here). Relative-span O(1) shift → deferred (Reuse Posture).
- **Reuse:** ports our own validated spike verbatim; `pulldown-cmark` is the parser.
- **Placeholder scan:** port tasks point to specific spike files/line-ranges (real validated code) + give concrete tests; no "TBD".
- **Type consistency:** `BlockKind`/`Block`/`BlockTree` (Task 1) consumed by `incremental_update` (Task 2) and the oracle (Task 3); names taken from the spike.

## Completion
When all tasks are `- [x]` and `cargo test --manifest-path wordcartel-core/Cargo.toml` is green (incl. the oracle): mark Plan 3a complete in the coverage ledger (§9.2 block_tree row → ✅). Then Plan 3b (block-role rendering: a `role_at`/per-line role query over the BlockTree, md_parse block-prefix conceal + block styling driven by `BlockRole`, and the live-preview integration).
