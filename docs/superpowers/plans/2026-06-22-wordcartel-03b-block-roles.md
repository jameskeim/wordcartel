# Wordcartel block-role rendering — Implementation Plan (Effort 3, Plan 3b)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make block-level Markdown constructs render in the live view: feed per-line block roles from `block_tree` into the `BlockRole` seam that `md_parse`/`layout` already expose, conceal block prefixes (`#`, `>`, list markers, code fences), and carry the block role through to the rendered rows so headings/lists/blockquotes/code/thematic-breaks display distinctly.

**Architecture:** `block_tree` (Plan 3a) provides a `role_at(byte) -> BlockRole` query. The editor computes a role per logical line and passes it to `layout(line, role, …)` → `md_parse::analyze(line, role, …)`, which now (when `!is_active`) conceals the line's block prefix and tags the visible content. `VisualRow` carries the line's `role` so the terminal renderer (a later effort) can paint heading weight/color, a blockquote gutter, etc. Headless; rendering = conceal + role metadata (no terminal here).

**Tech Stack:** Rust 2021; existing `pulldown-cmark 0.13`, `proptest`. Builds on the merged edit kernel, render core, and block_tree.

## Global Constraints

- Crate `wordcartel-core`; `#![forbid(unsafe_code)]`; pure/headless; byte-offset positions.
- Reuse the existing seam: `md_parse::analyze(line, role: BlockRole, is_active)`, `layout::layout(line, role, is_active, width)`, `style::BlockRole { Paragraph, Heading(u8), BlockQuote, ListItem, CodeBlock, ThematicBreak, FrontMatter }`.
- Enriching `BlockKind` (Task 1) changes `full_parse` output → the incremental==full **oracle must be re-run and still pass** (both parses capture the level identically).
- **v1 scope (decisions, flagged):** handle **top-level** block constructs (heading, blockquote, list, fenced/indented code, thematic break, paragraph). **Deferred:** deep-nested container *prefix stacking* (e.g. `> - item` conceals only the most-specific one prefix in v1), **hanging indent** for list continuation lines, and front-matter/HTML/table block styling (render their text as-is). Active line (`is_active`) shows raw — block prefixes are NOT concealed on the cursor's line (same rule as inline).
- TDD; pristine output; commit `proptest-regressions/` seeds.

---

## Reuse Posture
This is new rendering logic (not a port), but it plugs into seams already built: the `BlockRole` parameter md_parse/layout already accept (Plan 2), the `Style`/`StyledSeg` machinery, and `block_tree`'s tree. Only the role→prefix-conceal mapping and the role propagation are new.

---

## File Structure
- `wordcartel-core/src/block_tree.rs` — enrich `BlockKind::Heading` with level; add `BlockTree::role_at`.
- `wordcartel-core/src/style.rs` — (no change expected; `BlockRole` already has what we need).
- `wordcartel-core/src/md_parse.rs` — role-driven block-prefix conceal + bullet-glyph injection.
- `wordcartel-core/src/layout.rs` — `VisualRow` gains `role: BlockRole`; thematic-break row.
- `wordcartel-core/tests/block_roles_integration.rs` — block_tree.role_at → layout, multi-block live-preview.

---

### Task 1: Enrich BlockKind with heading level; re-gate the oracle

**Files:** `src/block_tree.rs`.

**Interfaces:** `BlockKind::Heading` becomes `Heading(u8)` (1–6). `tag_to_kind` maps `Tag::Heading { level, .. }` → `BlockKind::Heading(level as u8)` (pulldown `HeadingLevel::H1..H6` → 1..6).

> Note: this changes `full_parse` output, so the oracle's structural equality now includes the level. Both `full_parse` and `incremental_update` parse via the same `tag_to_kind`, so the oracle should still pass — Step 4 re-runs it.

- [ ] **Step 1:** Update the `tests` module in `block_tree.rs` — the existing `parses_heading_and_paragraph` test uses `BlockKind::Heading`; change it to `BlockKind::Heading(1)` and add a check for a level-3 heading:
```rust
#[test]
fn full_parse_captures_heading_level() {
    let t = full_parse("# H1\n\n### H3\n");
    assert_eq!(kinds(&t), vec![BlockKind::Heading(1), BlockKind::Heading(3)]);
}
```
(Update any other in-module test that constructs/expects `BlockKind::Heading` to use `Heading(n)`.)
- [ ] **Step 2:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml block_tree` → FAIL (variant arity changed / new test).
- [ ] **Step 3:** Change `BlockKind::Heading` → `Heading(u8)`; in `tag_to_kind`, map `Tag::Heading { level, .. }` to `BlockKind::Heading(level as u8)` (convert `pulldown_cmark::HeadingLevel` via `level as usize as u8`, which yields 1..6). Fix any match arms that referenced bare `Heading`.
- [ ] **Step 4:** Run the **oracle** explicitly: `cargo test --manifest-path wordcartel-core/Cargo.toml --test block_tree_oracle`. It MUST still pass (level is captured identically by full + incremental). If it FAILS, a parse path computes the level differently incrementally vs fully — fix it (do NOT weaken the oracle). Then run the full suite.
- [ ] **Step 5:** Commit: `feat(core): BlockKind::Heading carries level; oracle re-gated`

---

### Task 2: BlockTree::role_at(byte) -> BlockRole

**Files:** `src/block_tree.rs`.

**Interfaces — Produces:** `pub fn role_at(&self, byte: usize) -> crate::style::BlockRole` on `BlockTree`.

**Semantics:** Walk the tree; among all blocks whose `span` **contains** `byte`, pick the role by this precedence (most specific block-level construct wins): `FencedCode|IndentedCode → CodeBlock`; `Heading(n) → Heading(n)`; `ThematicBreak → ThematicBreak`; `ListItem → ListItem`; `BlockQuote → BlockQuote`; else `Paragraph`. (HtmlBlock/Table/Other/FrontMatter → `Paragraph` for v1 — their text renders as-is.)

**Important clarifications (Codex review):**
- **Gaps don't tile the document.** Block spans are sparse — blank lines, trailing whitespace, and link-ref-def lines belong to NO block. A `byte` in a gap (or past EOF) is contained by no block → return **`Paragraph`** (the safe default; such lines have no prefix to conceal). Test the gap/boundary bytes explicitly.
- **Line-start query is the editor's usage, and it resolves to the OUTERMOST containing block for nested constructs.** The editor calls `role_at(line_start_byte)`. For a **top-level** construct (the v1 scope), the line-start byte is inside that block's span, so this is correct. For a **nested** construct queried at the line's first byte (e.g. `> # H` at byte 0), only the *container's* span (BlockQuote) contains byte 0 — the nested Heading span starts after `> ` — so `role_at` returns `BlockQuote`. **This is the accepted v1 behavior** (deep-nested prefix stacking is deferred): such a line renders with the outer container's role. The precedence above only changes the result when the queried byte is genuinely inside *both* spans (i.e. at the heading's content bytes).

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn role_at_classifies_blocks() {
    // doc: "# H\n\n> q\n\n- a\n\n```\nc\n```\n\n---\n\npara\n"
    let doc = "# H\n\n> q\n\n- a\n\n```\nc\n```\n\n---\n\npara\n";
    let t = full_parse(doc);
    use crate::style::BlockRole::*;
    let role = |needle: &str| t.role_at(doc.find(needle).unwrap());
    assert_eq!(role("H"), Heading(1));
    assert_eq!(role("q"), BlockQuote);     // line is inside a blockquote
    assert_eq!(role("a"), ListItem);       // line is a list item
    assert_eq!(role("c"), CodeBlock);      // inside a fenced code block
    assert_eq!(role("---"), ThematicBreak);
    assert_eq!(role("para"), Paragraph);
}

#[test]
fn role_at_gaps_and_boundaries_are_paragraph() {
    let doc = "# H\n\npara\n";
    let t = full_parse(doc);
    use crate::style::BlockRole::*;
    // the blank line (byte 4, the second '\n') is in a gap -> Paragraph
    assert_eq!(t.role_at(4), Paragraph);
    // a byte past document end -> Paragraph
    assert_eq!(t.role_at(doc.len() + 5), Paragraph);
}
```
- [ ] **Step 2:** Run → FAIL (no `role_at`).
- [ ] **Step 3: Implement** `role_at`: collect blocks containing `byte` (recursive walk over `top_level()` and `children`), reduce by the precedence above, map `BlockKind` → `BlockRole` (Heading(n)→Heading(n), etc.), default `Paragraph`.
- [ ] **Step 4:** Run → PASS.
- [ ] **Step 5:** Commit: `feat(core): BlockTree::role_at — per-byte BlockRole query`

---

### Task 3: md_parse block-prefix conceal (driven by role)

**Files:** `src/md_parse.rs`.

**Interfaces:** `analyze(line, role, is_active)` — when `!is_active`, additionally conceal the **block prefix** the `role` implies, and (for lists) emit a bullet glyph. `LineAnalysis` gains a new field `pub prefix_glyph: Option<String>` (e.g. `Some("• ".into())`) that the renderer prepends; the prefix concealment itself uses the existing per-byte `visible` grid.

**CONCEAL ORDERING (important — Codex review):** the block-prefix concealment must run **AFTER** the existing inline conceal + reveal + escape passes (so a heading's `#` cannot be re-revealed by the inline reveal pass), i.e. set the block-prefix bytes `visible = false` as the LAST mutation of the grid, immediately **before** `collapse_runs`. This composes with inline styling: `## **bold**` → the `## ` prefix is hidden AND the `**` is hidden, leaving `bold` with `Style::Strong`.

**Prefix rules (apply only when `!is_active`):**
- `Heading(_)`: if the line starts (after optional indent) with `#{1,6}` followed by a space → conceal that ATX marker (visible = the heading text). **Setext support:** else if the line is a setext **underline** (matches `^\s*[=-]+\s*$`) → conceal the whole line (it's the `===`/`---` under a setext heading; `role_at` returns `Heading` for it because it is inside the heading block). Else (a setext heading *text* line — no `#`, not an underline) → conceal nothing; the text shows, styled as a heading via the row role (Task 4).
- `BlockQuote`: conceal one leading `>` and an optional following space (v1: one level only — see scope note).
- `ListItem`: conceal the leading marker — `[-*+] ` (unordered) or `\d+[.)] ` (ordered) — and set `prefix_glyph = Some("• ".to_string())` for unordered, or the ordinal text (e.g. `"1. "`) for ordered. (Hanging indent deferred.)
- `CodeBlock`: if the line is a fence (`^\s*(```|~~~)`), conceal the whole fence line; otherwise leave content visible (the renderer styles it via the row role).
- `ThematicBreak`: conceal the whole line (the renderer draws a rule from the row role — Task 4); set `prefix_glyph = None`.
- `Paragraph` / others: no prefix conceal.

Implement by scanning the line's leading bytes for the role's prefix (regex-free byte scanning; the matches above are simple byte patterns). The block-prefix bytes are ASCII, so concealment is char-boundary-safe.

- [ ] **Step 1: Write failing tests** (add `prefix_glyph` to the analysis type usage):
```rust
#[test]
fn heading_prefix_concealed() {
    let a = analyze("## Title", BlockRole::Heading(2), false);
    assert_eq!(visible(&a, "## Title"), "Title"); // "## " hidden
}
#[test]
fn blockquote_prefix_concealed() {
    let a = analyze("> quoted", BlockRole::BlockQuote, false);
    assert_eq!(visible(&a, "> quoted"), "quoted");
}
#[test]
fn list_marker_becomes_bullet_glyph() {
    let a = analyze("- item", BlockRole::ListItem, false);
    assert_eq!(visible(&a, "- item"), "item");
    assert_eq!(a.prefix_glyph.as_deref(), Some("• "));
}
#[test]
fn fence_line_concealed() {
    let a = analyze("```rust", BlockRole::CodeBlock, false);
    assert_eq!(visible(&a, "```rust"), ""); // fence line hidden
}
#[test]
fn active_line_keeps_block_prefix_raw() {
    let a = analyze("## Title", BlockRole::Heading(2), true);
    assert_eq!(visible(&a, "## Title"), "## Title"); // raw on cursor line
    assert!(a.prefix_glyph.is_none());
}
#[test]
fn heading_prefix_composes_with_inline_style() {
    // "## **bold**" -> "## " AND "**" hidden, leaving "bold" as Strong.
    let line = "## **bold**";
    let a = analyze(line, BlockRole::Heading(2), false);
    assert_eq!(visible(&a, line), "bold");
    // 'b' is at byte 5 in "## **bold**"
    assert_eq!(style_at(&a, 5), Some(Style::Strong));
}
#[test]
fn list_marker_composes_with_inline_style() {
    let line = "- **item**";
    let a = analyze(line, BlockRole::ListItem, false);
    assert_eq!(visible(&a, line), "item");
    assert_eq!(a.prefix_glyph.as_deref(), Some("• "));
}
#[test]
fn setext_underline_concealed() {
    // role_at returns Heading for the underline line; conceal the whole "---".
    let a = analyze("---", BlockRole::Heading(1), false);
    assert_eq!(visible(&a, "---"), "");
}
```
(`visible` and `style_at` are the existing test helpers in `md_parse`'s test module.)
- [ ] **Step 2:** Run `cargo test ... md_parse` → FAIL.
- [ ] **Step 3:** Add `pub prefix_glyph: Option<String>` to `LineAnalysis` in `style.rs`, then update **every** `LineAnalysis { .. }` construction site so it compiles: md_parse's active-line early return (set `prefix_glyph: None`), md_parse's normal return (set the computed glyph), and the `style.rs` `types_construct` test literal. Implement the role-driven prefix conceal + glyph per the rules above (apply the prefix conceal as the LAST grid mutation before `collapse_runs`, per CONCEAL ORDERING). The active-line early return conceals nothing and sets `prefix_glyph: None`.
- [ ] **Step 4:** Run → PASS. Re-run the full `md_parse` + `layout` suites (the new field must not break existing tests / the 6 layout property laws — `layout` reads `analysis.runs`/`styles`, unaffected by `prefix_glyph`).
- [ ] **Step 5:** Commit: `feat(core): md_parse role-driven block-prefix conceal + bullet glyph`

---

### Task 4: VisualRow carries role; thematic-break row

**Files:** `src/layout.rs`.

**Interfaces:** `VisualRow` gains `pub role: BlockRole` (the line's block role, copied from `layout`'s `role` arg onto every row of the line) and `pub prefix_glyph: Option<String>` (taken from the `LineAnalysis`, attached to the FIRST visual row of the line only). This is the block-style channel for the future terminal renderer.

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn rows_carry_block_role_and_glyph() {
    let (rows, _m) = layout("- item", BlockRole::ListItem, false, 80);
    assert_eq!(rows[0].role, BlockRole::ListItem);
    assert_eq!(rows[0].prefix_glyph.as_deref(), Some("• "));
}
#[test]
fn heading_rows_carry_heading_role() {
    let (rows, _m) = layout("## Title", BlockRole::Heading(2), false, 80);
    assert!(rows.iter().all(|r| r.role == BlockRole::Heading(2)));
}
```
- [ ] **Step 2:** Run `cargo test ... layout` → FAIL.
- [ ] **Step 3:** Add `role: BlockRole` and `prefix_glyph: Option<String>` fields to `VisualRow` (keep its `#[derive(Clone, Debug, PartialEq, Eq)]`). There is a single `VisualRow` construction site — the `vec![VisualRow { … }; rows]` initializer in `layout` — update it to initialize both fields (`role`, `prefix_glyph: None`). After the rows are assembled, set every row's `role = role` and `rows[0].prefix_glyph = analysis.prefix_glyph` (first row only). Keep all soft-wrap/ColMap/segs logic unchanged. (Existing layout tests inspect `display`/`width`/`segs`, not full `VisualRow` equality, so they keep compiling; the 6 property laws use `BlockRole::Paragraph` and don't read the new fields.)
- [ ] **Step 4:** Run → PASS; re-run the full `layout` suite incl. the 6 property laws (they don't inspect `role`/`prefix_glyph`, so they remain green; the `VisualRow` `PartialEq` change just adds fields).
- [ ] **Step 5:** Commit: `feat(core): VisualRow carries block role + prefix glyph`

---

### Task 5: Block-role live-preview integration

**Files:** Create `wordcartel-core/tests/block_roles_integration.rs`.

**Interfaces:** the cross-module law that `block_tree.role_at` + `layout` produce a correct block-styled live preview of a multi-block document.

- [ ] **Step 1: Write the failing test:**
```rust
//! End-to-end block-role rendering: parse a multi-block doc, derive each logical
//! line's role from block_tree, lay it out, and verify prefixes are concealed and
//! roles/glyphs are correct.
use wordcartel_core::block_tree::full_parse;
use wordcartel_core::layout::layout;
use wordcartel_core::style::BlockRole;

#[test]
fn multi_block_doc_renders_with_roles() {
    let doc = "# Title\n\n> a quote\n\n- first\n- second\n\nplain para\n";
    let tree = full_parse(doc);
    // iterate logical lines (\n-delimited), compute each line's role at its start byte.
    let mut offset = 0usize;
    let mut got: Vec<(BlockRole, String, Option<String>)> = Vec::new();
    for line in doc.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if !trimmed.is_empty() {
            let role = tree.role_at(offset);
            let (rows, _m) = layout(trimmed, role, false, 80);
            let display: String = rows.iter().map(|r| r.display.clone()).collect();
            got.push((role, display, rows[0].prefix_glyph.clone()));
        }
        offset += line.len();
    }
    // Title heading: "# " concealed -> "Title", role Heading(1)
    assert_eq!(got[0], (BlockRole::Heading(1), "Title".into(), None));
    // quote: "> " concealed
    assert_eq!(got[1], (BlockRole::BlockQuote, "a quote".into(), None));
    // list items: marker -> bullet glyph
    assert_eq!(got[2], (BlockRole::ListItem, "first".into(), Some("• ".into())));
    assert_eq!(got[3], (BlockRole::ListItem, "second".into(), Some("• ".into())));
    // paragraph: unchanged
    assert_eq!(got[4], (BlockRole::Paragraph, "plain para".into(), None));
}
```
- [ ] **Step 2:** Run `cargo test --manifest-path wordcartel-core/Cargo.toml --test block_roles_integration`. It should pass once Tasks 1–4 are in. If it FAILS, fix the offending module (role_at precedence, prefix conceal, or row role propagation) — not the test.
- [ ] **Step 3:** Run the full suite (no regressions); commit: `test(core): block-role live-preview integration`

---

## Self-Review (completed during planning)

- **Spec coverage:** §13.4 block construct rendering (heading/list/quote/code/thematic) → Tasks 3–5; the final-review handoff (BlockKind heading level + role query) → Tasks 1–2; the Plan-2 seam wiring → Tasks 3–5. Hanging indent, deep-nested prefix stacking, HTML/table/front-matter block styling → deferred (Global Constraints), flagged.
- **Reuse:** plugs into the existing `BlockRole`/`md_parse`/`layout`/`block_tree` seams; only the role→conceal mapping + propagation is new.
- **Oracle safety:** Task 1 changes `full_parse` output (heading level) and explicitly re-gates the incremental==full oracle (Step 4).
- **Placeholder scan:** all tasks have concrete tests with expected values; no "TBD".
- **Type consistency:** `BlockKind::Heading(u8)` (Task 1) → `role_at` mapping (Task 2) → `BlockRole::Heading(u8)` consumed by md_parse (Task 3) and carried on `VisualRow` (Task 4) and asserted in integration (Task 5). `prefix_glyph: Option<String>` introduced on `LineAnalysis` (Task 3) and `VisualRow` (Task 4).

## Completion
When all tasks are `- [x]` and the full suite (incl. the re-gated oracle) is green: mark Plan 3b complete; flip the §13 / §3.2 live-conceal-render-modes ledger rows toward ✅. This completes Effort 3 (block_tree + block-role rendering). Next effort: IO/Shell (crossterm input, ratatui render, clipboard, atomic save, filter, repar).
