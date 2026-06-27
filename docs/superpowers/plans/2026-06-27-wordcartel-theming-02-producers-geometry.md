# Theming Plan ② — Producers, Cursor-Safe Prefix Geometry & §13.2

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make front matter, comments, blockquotes, thematic breaks, and heading-levels themable + cursor-safe, paint the document selection, and prove §13.2 — building on plan ①'s theme model (already merged on this branch).

**Architecture:** Two core *producers* (byte-0 YAML front matter → `BlockKind::FrontMatter`; `<!-- -->` comments → `BlockKind::HtmlComment` + inline `Style::Comment`) plus structural glyphs (blockquote `▎`, thematic-break `───`). The **keystone**: route every row prefix through `layout`'s visual geometry — add `ColMap.prefix_width`, offset `Placed.col` and reduce wrap capacity, so cursor (`source_to_visual`) and mouse (`visual_to_source`) account for the prefix automatically (fixing the latent list-bullet desync). Then document-selection painting and the §13.2 coverage proof.

**Tech Stack:** Rust, `wordcartel-core` (`#![forbid(unsafe_code)]`, IO-free), `wordcartel` shell, pulldown-cmark 0.13, ratatui 0.30.

## Global Constraints
- `wordcartel-core` is `#![forbid(unsafe_code)]`, IO/thread-free, NO ratatui dep. The theme model + `compose` seam from **plan ① are already on this branch** — consume them, don't rebuild.
- **Front matter is byte-0 ONLY.** Do NOT enable pulldown's global metadata option (it would let the incremental reparser misclassify a mid-document `--- … ---`, breaking full/incremental oracle equivalence). Detect a leading `---\n…\n---` block; force a reparse-from-byte-0 for any edit intersecting the front-matter block span / opening delimiter / a possible closing delimiter (broaden beyond delimiters — Codex). Oracle must cover: mid-doc `--- … ---` stays thematic-break/setext (NOT front matter), and front-matter-body edits stay full==incremental.
- **Comments:** `BlockKind::HtmlComment` only when a block's source begins `<!--` (do NOT color `<div>` as a comment); inline `Style::Comment` only when an `Event::InlineHtml` source span is `<!-- … -->`.
- **Cursor-safe prefix geometry (the keystone):** all synthetic prefixes (list bullet, blockquote `▎`, thematic-break `───`, heading-level glyph) are width-accounted in `layout`/`ColMap` so `nav::source_to_visual`/`visual_to_source`, cursor placement, and `mouse::offset_at_cell` are correct. Round-trip cursor/mouse tests on prefixed + wrapped + narrow rows. (This intentionally gives wrapped prefixed lines a hanging indent — a deliberate, accepted look change; update those goldens.)
- **Heading-level glyph** is theme-driven (`theme.heading_level_glyph`): `layout` takes a `heading_prefix: bool` input reserving a FIXED width when on (so ColMap geometry stays theme-independent for a given setting); a theme switch relayouts (plan ① already re-seeds; this plan adds the relayout-on-switch is plan ③'s picker — here the glyph is driven by the seeded theme).
- **Selection painting:** the primary selection range paints (layer the `Selection` face = reverse) on document cells in BOTH render paths; a non-empty selection forces the placed path; selection is read from `editor.active().document.selection.primary()`.
- **§13.2:** every `SemanticElement` carries a non-color cue in cue mode (No-color theme OR phosphor-flat); the proof is a `TestBackend` render coverage table run in BOTH cue-mode themes + pairwise collision tests for the same-context persistent pairs.
- Responsiveness #1: no per-keystroke heavy work; the prefix width is computed in the existing per-frame `layout` pass.

---

## File Structure
- **Modify** `wordcartel-core/src/style.rs` — `Style::Comment`.
- **Modify** `wordcartel-core/src/md_parse.rs` — inline `<!-- -->`→`Style::Comment`; blockquote `▎` + thematic-break `───` glyphs.
- **Modify** `wordcartel-core/src/block_tree.rs` — `BlockKind::{FrontMatter, HtmlComment}` + `kind_to_role` + the byte-0 front-matter detection + `frontmatter_in_play` reparse trigger.
- **Modify** `wordcartel-core/src/layout.rs` — `ColMap.prefix_width`; offset `Placed.col` + reduce wrap capacity; `visual_to_source` prefix clamp; the `heading_prefix` input.
- **Modify** `wordcartel/src/nav.rs` — `layout(...)` call sites get `heading_prefix`; `typewriter_rows_of_line` prefix-aware.
- **Modify** `wordcartel/src/render.rs` — `style_to_ratatui`/`style_element` Comment arms; selection painting; the heading-level glyph fill; role→element for FrontMatter/Comment.
- **Modify** `wordcartel/src/derive.rs` — `layout(...)` call gets `heading_prefix` (from `editor.theme.heading_level_glyph`).

---

## Task 1: Core — `Style::Comment` + exhaustive-match total test

**Files:** `wordcartel-core/src/style.rs`; `wordcartel/src/render.rs`; `wordcartel-core/src/md_parse.rs` (test).

**Interfaces:** Produces `Style::Comment` (8th variant). All exhaustive `match Style` sites gain an arm.

- [ ] **Step 1: Write the failing test** (in `style.rs` `#[cfg(test)]`)
```rust
    #[test]
    fn style_comment_exists() {
        let _ = Style::Comment;
        // total: a match over Style must be able to name Comment (compile-guard).
        fn _exhaustive(s: Style) -> u8 { match s {
            Style::Plain=>0, Style::Emphasis=>1, Style::Strong=>2, Style::StrongEmphasis=>3,
            Style::Code=>4, Style::Strikethrough=>5, Style::Link=>6, Style::Comment=>7 } }
    }
```

- [ ] **Step 2: Run — fails to compile** (`Style::Comment` undefined).
Run: `cargo test -p wordcartel-core style_comment_exists`

- [ ] **Step 3: Add the variant + fix the 2 render match arms**

`style.rs:5`:
```rust
pub enum Style { Plain, Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link, Comment }
```
`render.rs` `style_to_ratatui` (add arm; this is the legacy mapper still kept for one test): `Style::Comment => RStyle::default().add_modifier(Modifier::DIM).add_modifier(Modifier::ITALIC),`
`render.rs` `style_element`: `Style::Comment => SE::Comment,`
(`md_parse::current_style` does NOT need an arm — Comment isn't produced by the nesting counters; it's emitted directly in Task 2.)

- [ ] **Step 4: Run** `cargo test -p wordcartel-core && cargo build -p wordcartel` — PASS + compiles (the 2 render arms make it exhaustive).

- [ ] **Step 5: Commit** `feat(theme): Style::Comment variant + exhaustive match arms`

---

## Task 2: Core — inline `<!-- -->` → `Style::Comment`

**Files:** `wordcartel-core/src/md_parse.rs` (the `analyze` event loop ~line 92); test.

**Interfaces:** Consumes `Style::Comment`. An inline HTML comment emits a `StyleSpan{Style::Comment}` + conceals nothing extra (the comment text shows, styled).

- [ ] **Step 1: Write the failing test**
```rust
    #[test]
    fn inline_html_comment_is_styled_comment() {
        let a = analyze("text <!-- note --> more", BlockRole::Paragraph, false);
        let cmt = a.styles.iter().find(|s| s.style == Style::Comment).expect("comment span");
        // span covers the `<!-- note -->`
        assert_eq!(&"text <!-- note --> more"[cmt.src.clone()], "<!-- note -->");
    }
    #[test]
    fn inline_html_non_comment_tag_is_not_comment() {
        let a = analyze("a <span>x</span> b", BlockRole::Paragraph, false);
        assert!(a.styles.iter().all(|s| s.style != Style::Comment), "a <span> is not a comment");
    }
```

- [ ] **Step 2: Run — fails** (no Comment span). `cargo test -p wordcartel-core inline_html`

- [ ] **Step 3: Add the `Event::InlineHtml` arm** in `analyze`'s match, BEFORE the `_ => {}` catch-all:
```rust
            Event::InlineHtml(_) => {
                // Style a `<!-- … -->` inline comment; leave other inline HTML (<span> etc.) Plain.
                let s = &line[range.clone()];
                if s.starts_with("<!--") && s.ends_with("-->") {
                    reveal.push(range.clone());          // keep the comment visible
                    styles.push(StyleSpan { src: range, style: Style::Comment });
                }
            }
```
(`Event::InlineHtml` exists in pulldown-cmark 0.13; `range` is the source byte range in `line`. We `reveal` so the comment text shows, styled — consistent with how Code reveals inner content.)

- [ ] **Step 4: Run** `cargo test -p wordcartel-core md_parse:: inline_html` — PASS; full core suite green.

- [ ] **Step 5: Commit** `feat(theme): inline <!-- --> → Style::Comment producer`

---

## Task 3: Core — block `<!-- -->` → `BlockKind::HtmlComment` → `BlockRole::Comment`

**Files:** `wordcartel-core/src/block_tree.rs` (`BlockKind`, `tag_to_kind`, `kind_to_role`); test.

**Interfaces:** Produces `BlockKind::HtmlComment` (a block whose source begins `<!--`); `kind_to_role(HtmlComment) = Comment`. Generic `HtmlBlock` stays `None` (unchanged).

- [ ] **Step 1: Write the failing test**
```rust
    #[test]
    fn block_html_comment_maps_to_comment_role() {
        let doc = "<!-- a block comment -->\n\npara\n";
        let t = full_parse(doc);
        let at = |needle: &str| t.role_at(doc.find(needle).unwrap());
        assert_eq!(at("block comment"), crate::style::BlockRole::Comment);
    }
    #[test]
    fn block_div_is_not_comment() {
        let doc = "<div>x</div>\n\npara\n";
        let t = full_parse(doc);
        assert_ne!(t.role_at(doc.find("x").unwrap()), crate::style::BlockRole::Comment);
    }
```

- [ ] **Step 2: Run — fails.** `cargo test -p wordcartel-core block_html_comment block_div`

- [ ] **Step 3: Implement.** (a) `BlockKind` gains `HtmlComment`:
```rust
pub enum BlockKind { Document, Paragraph, Heading(u8), FencedCode, IndentedCode,
    BlockQuote, List, ListItem, ThematicBreak, HtmlBlock, HtmlComment, Table, Other }
```
(b) `tag_to_kind` maps `Tag::HtmlBlock` to `HtmlBlock` as before — but the COMMENT discrimination needs the source. The cleanest seam: in `parse_region`, when pushing an `HtmlBlock` block, peek the block's source: if `src.slice(span).trim_start().starts_with("<!--")`, push `HtmlComment` instead. Concretely, in the `Event::Start(tag)` arm, after `tag_to_kind`, special-case:
```rust
            Event::Start(tag) => {
                if let Some(kind) = tag_to_kind(&tag) {
                    let kind = if kind == BlockKind::HtmlBlock
                        && text[range.clone()].trim_start().starts_with("<!--") {
                        BlockKind::HtmlComment
                    } else { kind };
                    stack.push(Block { kind, span, children: Vec::new() });
                }
            }
```
(`text` is the region slice already bound in `parse_region`; `range` is region-local, so index `text[range]` not the base-shifted `span`.) (c) `kind_to_role` adds `BlockKind::HtmlComment => Some(BlockRole::Comment),`. (d) **`BlockRole::Comment` does NOT exist (Codex I6) — add it + every exhaustive match site:**
   - `wordcartel-core/src/style.rs:8` — add `Comment` to `pub enum BlockRole { … }`.
   - `wordcartel-core/src/block_tree.rs::role_precedence` (~:193) — give `Comment` a precedence **lower (numerically) than `Paragraph`** so `collect_role` (which updates only on strictly-lower precedence, ~:226) lets it win over the default Paragraph. (This is the Codex-C2 class — without it `role_at` returns Paragraph, not Comment.)
   - `wordcartel/src/render.rs::role_element` (~:68) — add `BlockRole::Comment => SE::Comment,`.
   - `grep -rn "BlockRole::" wordcartel-core/ wordcartel/` for any OTHER exhaustive `match` on `BlockRole` (md_parse `apply_block_prefix_conceal` matches BlockRole — add a `BlockRole::Comment => None` arm there; any layout/derive match) and add the arm so it compiles.

- [ ] **Step 4: Run** `cargo test -p wordcartel-core block_tree:: block_html_comment block_div` + the oracle suite `cargo test -p wordcartel-core --test block_tree_oracle` — PASS (HtmlComment doesn't change incremental behavior; HtmlBlock was already full-reparse-on-`<`).

- [ ] **Step 5: Commit** `feat(theme): block <!-- --> → HtmlComment → BlockRole::Comment`

---

## Task 4: Core — byte-0 YAML front matter → `BlockKind::FrontMatter`

**Files:** `wordcartel-core/src/block_tree.rs` (`BlockKind`, byte-0 detection in full + incremental, `kind_to_role`, a `frontmatter_in_play` trigger); `tests/block_tree_oracle.rs`; test.

**Interfaces:** Produces `BlockKind::FrontMatter` for a leading `---\n…\n---` block; `kind_to_role(FrontMatter) = BlockRole::FrontMatter`. Edits touching the front-matter span force a reparse-from-byte-0.

- [ ] **Step 1: Write the failing tests**
```rust
    #[test]
    fn byte0_front_matter_is_front_matter_role() {
        let doc = "---\ntitle: Hi\n---\n\n# Heading\n";
        let t = full_parse(doc);
        assert_eq!(t.role_at(doc.find("title").unwrap()), crate::style::BlockRole::FrontMatter);
        // the heading after it is unaffected
        assert_eq!(t.role_at(doc.find("Heading").unwrap()), crate::style::BlockRole::Heading(1));
    }
    #[test]
    fn mid_document_dashes_are_not_front_matter() {
        // a `---` NOT at byte 0 is a thematic break / setext underline, never front matter.
        let doc = "para\n\n---\n\nmore\n";
        let t = full_parse(doc);
        assert_ne!(t.role_at(doc.find("more").unwrap()), crate::style::BlockRole::FrontMatter);
    }
```

- [ ] **Step 2: Run — fails.** `cargo test -p wordcartel-core byte0_front_matter mid_document_dashes`

- [ ] **Step 3: Implement byte-0 detection (full parse) + role.**
(a) `BlockKind` gains `FrontMatter`. (b) A pure helper:
```rust
/// If `src` begins with a YAML front-matter block (`---\n … \n---`), return its
/// byte range (the whole block incl. both fences); else None. Byte-0 ONLY.
fn front_matter_span(src: &str) -> Option<Range<usize>> {
    let rest = src.strip_prefix("---\n")?;            // opening fence MUST be at byte 0
    // closing fence: a line that is exactly "---" (or "...") — find the first.
    let mut off = 4; // after "---\n"
    for line in rest.split_inclusive('\n') {
        let trimmed = line.strip_suffix('\n').unwrap_or(line);
        if trimmed == "---" || trimmed == "..." {
            return Some(0..off + line.len());
        }
        off += line.len();
    }
    None
}
```
(c) **Detect ONLY in the whole-document entry point, never in `parse_region` keyed on `base == 0` (Codex C3).** The incremental splice calls `parse_region(&new_region, 0..new_region.len(), region_new_start)` — `base`/the region-start arg can be `0` for a *localized* edit at doc start, so a `base == 0` check would wrongly run the byte-0 scanner on a splice fragment. Instead put the front-matter detection in the `full_parse`/`full_parse_rope`/`full_parse_src` wrapper (the only true whole-document caller): BEFORE it calls into the event loop, if `front_matter_span(text)` is `Some(fm)`, push a `Block{ kind: FrontMatter, span: fm.clone(), children: vec![] }`, then parse the REMAINDER `text[fm.end..]` (run the existing region parse over `&text[fm.end..]` with a base offset of `fm.end`, shifting spans by `fm.end`) and append those blocks. `parse_region` itself stays front-matter-blind. (Keep the non-front-matter path unchanged.) `front_matter_span` returns None for a mid-doc `---` because `strip_prefix("---\n")` requires byte 0. (d) `kind_to_role`: `BlockKind::FrontMatter => Some(BlockRole::FrontMatter),`. (e) **Role precedence (Codex C2):** `role_precedence` gives `Paragraph` and `FrontMatter` the same rank, and `collect_role` only overrides on *strictly-lower* precedence — so without a change the pre-existing `Paragraph` role wins and `role_at` never returns `FrontMatter`. Give `FrontMatter` a precedence **numerically lower than `Paragraph`** in `role_precedence` so it wins. (The Step-1 `role_at` test is the gate — it would fail without this.)

- [ ] **Step 4: Front-matter-aware incremental reparse + oracle.**
The incremental splice reparses a localized region from a non-zero base — a byte-0 front-matter parser can't run there. Add a `frontmatter_in_play` trigger (mirroring `html_in_play`) that forces a reparse-from-byte-0 when the edit could affect front matter:
```rust
/// Force a full (byte-0) reparse when an edit touches the front-matter region:
/// any edit intersecting an existing/possible leading `---` block, or inserting
/// `---` near byte 0. Conservative: if byte 0 of either old or new src is within
/// or adjacent to a `---` fence affected by the edit, reparse from 0.
fn frontmatter_in_play<S: TextSource>(old_src: &S, new_src: &S, edit: &Edit) -> bool {
    let touches_head = edit.range.start <= front_matter_span(&old_src.slice(0..old_src.len())).map(|f| f.end).unwrap_or(0).max(4);
    // also: the edit reaches into the first few lines where a fence could form
    touches_head || edit.range.start <= 4 || {
        let new = new_src.slice(0..new_src.len());
        front_matter_span(new.as_ref()).map(|f| edit.range.start <= f.end).unwrap_or(false)
    }
}
```
> The implementer: wire this beside the existing `html_in_play` check in the incremental update path. A true result MUST route to a real whole-document reparse — i.e. call the same `full_parse_*` entry point that does the byte-0 front-matter detection from (c) (NOT a widened-but-still-localized `parse_region` splice, which stays front-matter-blind). If the existing `html_in_play` branch already re-runs full parse, reuse it; otherwise add the `frontmatter_in_play(..) => full_parse_src(new_src)` short-circuit. The exact `Edit` field names (`edit.range`, `edit.delta()`) and the `TextSource::slice` signature are in the surrounding code — match them. The CONTRACT is the oracle below; get the trigger broad enough that every oracle case passes (when in doubt, reparse from 0 — front matter is a tiny region at the doc head, so the cost is negligible).

Add oracle cases to `tests/block_tree_oracle.rs` (use the existing `check`/macro pattern):
```rust
    // front matter: editing a body value stays full==incremental
    check("---\ntitle: a\n---\n\npara\n", /* edit "a"→"bb" */ 11..12, "bb");
    // inserting a line inside front matter
    check("---\nt: a\n---\n\np\n", 7..7, "x: y\n");
    // a mid-doc `---` must NOT become front matter after an unrelated edit
    check("p\n\n---\n\nq\n", 8..9, "Q");
    // typing the opening fence at byte 0 turns the head into front matter
    check("title: a\n\np\n", 0..0, "---\n");
```
(adapt the exact byte ranges to the real strings; each asserts incremental == full via `check`.)

- [ ] **Step 5: Run** `cargo test -p wordcartel-core block_tree:: byte0_front_matter mid_document_dashes` + `cargo test -p wordcartel-core --test block_tree_oracle` — ALL PASS (full==incremental for every front-matter case).

- [ ] **Step 6: Commit** `feat(theme): byte-0 YAML front matter → BlockKind::FrontMatter + reparse-from-0 trigger`

---

## Task 5: Core — blockquote `▎` + thematic-break `───` glyphs

**Files:** `wordcartel-core/src/md_parse.rs` (`apply_block_prefix_conceal`); `wordcartel/src/render.rs` (prefix-glyph styling); test.

**Interfaces:** `apply_block_prefix_conceal` returns `Some("▎ ")` for BlockQuote and `Some("─── ")` (or a full-width rule) for ThematicBreak (previously both `None`). Render styles each `vr.prefix_glyph` by its block role, not always `ListMarker`.

- [ ] **Step 1: Write the failing test**
```rust
    #[test]
    fn blockquote_has_bar_glyph() {
        let a = analyze("> quoted", BlockRole::BlockQuote, false);
        assert_eq!(a.prefix_glyph.as_deref(), Some("▎ "));
        assert_eq!(visible(&a, "> quoted"), "quoted"); // existing conceal still holds
    }
    #[test]
    fn thematic_break_has_rule_glyph() {
        let a = analyze("---", BlockRole::ThematicBreak, false);
        assert_eq!(a.prefix_glyph.as_deref(), Some("─── "));
    }
```
(`visible(&a, src)` is the existing test helper that reconstructs the visible string — mirror its use in the existing blockquote conceal test.)

- [ ] **Step 2: Run — fails** (both return None). `cargo test -p wordcartel-core blockquote_has_bar thematic_break_has_rule`

- [ ] **Step 3: Implement (core glyphs).** In `apply_block_prefix_conceal`, the `BlockRole::BlockQuote` arm: after concealing `>` + space, `return Some("▎ ".to_string());` (was `None`). The `BlockRole::ThematicBreak` arm: after concealing the line, `return Some("─── ".to_string());` (was `None`).

- [ ] **Step 3b: Style the prefix glyph by role (render — Codex I4).** Render currently styles EVERY `vr.prefix_glyph` with `SE::ListMarker` (~`render.rs:331`/`:373`). With Task 5's new glyphs (and Task 7's heading placeholder), that would paint the blockquote `▎` and thematic `───` as list markers. Add a small mapping and use it where the prefix glyph is painted:
```rust
fn prefix_element(role: BlockRole) -> SemanticElement {
    match role {
        BlockRole::BlockQuote => SemanticElement::BlockQuote,
        BlockRole::ThematicBreak => SemanticElement::ThematicBreak,
        BlockRole::Heading(n) => SemanticElement::Heading(n),
        _ => SemanticElement::ListMarker,
    }
}
```
Paint the prefix glyph span with `compose([prefix_element(vr.role)])` instead of the hardcoded `ListMarker`. (Heading arm is exercised in Task 7; list/other fall through to `ListMarker` exactly as today — no behavior change for existing list bullets.)
- [ ] **Step 3c: Render gate test.** Add a shell render test: a blockquote line paints its `▎` cell with the `BlockQuote` face (not the list-marker face) under the Default theme. Mirror an existing prefix-glyph render assertion.

- [ ] **Step 4: Run** `cargo test -p wordcartel-core md_parse:: blockquote thematic` + `cargo test -p wordcartel render::` — PASS. Then `cargo test -p wordcartel-core` — fix any EXISTING md_parse test that asserted blockquote/thematic prefix_glyph == None (those goldens are intentionally updated: the glyphs are new). Update such a test to expect the glyph.

- [ ] **Step 5: Commit** `feat(theme): blockquote ▎ + thematic-break ─── prefix glyphs + role-styled prefixes`

---

## Task 6: KEYSTONE — `ColMap.prefix_width` + cursor-safe layout

**Files:** `wordcartel-core/src/layout.rs` (`ColMap`, `layout`, `visual_to_source`); test.

**Interfaces:**
- `ColMap` gains `pub prefix_width: usize` (display width of the row's prefix glyph; 0 if none).
- `layout` offsets every `Placed.col` by `prefix_width` and wraps at `viewport_width` with effective capacity `viewport_width - prefix_width` (uniform hanging indent for continuation rows).
- `visual_to_source(row, col)` clamps `col < prefix_width` to `prefix_width` (a click on the prefix lands at the line's first text glyph).

- [ ] **Step 1: Write the failing tests**
```rust
    fn gwidth(s: &str) -> usize { s.chars().map(|_| 1).count() } // ascii test glyphs; real uses grapheme_width
    #[test]
    fn prefix_offsets_columns_so_cursor_lands_on_text() {
        // a list item: prefix "• " (width 2). The first text glyph 'i' must be at col 2, not 0.
        let (_rows, map) = layout("- item", BlockRole::ListItem, false, 40);
        assert_eq!(map.prefix_width, 2, "• + space");
        let (row, col) = map.source_to_visual(2); // byte 2 = 'i' (after "- ")
        assert_eq!((row, col), (0, 2));
        // a click in the prefix region (col 0/1) lands at the first text byte, not end-of-row
        assert_eq!(map.visual_to_source(0, 0), map.visual_to_source(0, 2));
    }
    #[test]
    fn no_prefix_is_unchanged() {
        let (_rows, map) = layout("plain text", BlockRole::Paragraph, false, 40);
        assert_eq!(map.prefix_width, 0);
        assert_eq!(map.source_to_visual(0), (0, 0)); // no offset
    }
    #[test]
    fn prefix_reduces_wrap_capacity() {
        // width-6 viewport, prefix width 2 → text wraps after 4 cols, continuation indented to col 2.
        let (rows, map) = layout("- aaaa bbbb", BlockRole::ListItem, false, 6);
        assert!(map.rows >= 2, "should wrap");
        // a glyph on row 1 starts at col == prefix_width (hanging indent)
        let first_row1 = map.placed.iter().find(|p| p.row == 1 && p.width > 0).unwrap();
        assert_eq!(first_row1.col, 2, "continuation indented to prefix_width");
    }
```

- [ ] **Step 2: Run — fails** (no `prefix_width` field). `cargo test -p wordcartel-core prefix_offsets no_prefix_is_unchanged prefix_reduces`

- [ ] **Step 3: Implement** (in `layout.rs`):
(a) Add `pub prefix_width: usize` to `ColMap` (and its construction at the end of `layout`).
(b) Compute the prefix width before the wrap loop:
```rust
    let prefix_glyph = analysis.prefix_glyph.clone();
    use unicode_segmentation::UnicodeSegmentation;
    let prefix_width: usize = prefix_glyph.as_deref()
        .map(|g| g.graphemes(true).map(grapheme_width).sum()) // real helper, over graphemes
        .unwrap_or(0);
```
> Use the SAME display-width function the layout already uses for cells — the real helper is `layout::grapheme_width(g: &str)` (Codex Minor 9), iterated over the prefix string's graphemes (`UnicodeSegmentation::graphemes`), NOT a `char` count — so `prefix_width` is the true painted width of the glyph string. The glyphs are `"• "` / `"▎ "` / `"─── "` (widths 2/2/4) and the heading glyph (Task 7).
(c) In the wrap loop, start `col` at `prefix_width` and wrap against the full `vw` (so capacity = `vw - prefix_width`), resetting to `prefix_width` on wrap:
```rust
    let mut col = prefix_width;
    ...
    for vg in &vgs {
        if vg.width == 0 { placed.push(Placed{ src: vg.src.clone(), row, col, width:0, text: vg.text.clone(), style: vg.style }); continue; }
        if col + vg.width > vw && col > prefix_width { row_end_col.push(col); row += 1; col = prefix_width; }
        placed.push(Placed{ src: vg.src.clone(), row, col, width: vg.width, text: vg.text.clone(), style: vg.style });
        col += vg.width;
    }
    row_end_col.push(col);
```
(d) `visual_to_source`: at the TOP, clamp the prefix region: `let col = col.max(self.prefix_width);` (so a click at col < prefix_width is treated as col == prefix_width → the first text glyph). This is the only ColMap-method change needed — `source_to_visual`/`col_on_row` read `Placed.col` which now includes the offset, so they're correct automatically.
(e) Set `prefix_width` in the returned `ColMap`.

(f) **The display-width fn (Codex Minor 9):** the real helper is `layout::grapheme_width(g: &str)` (uses `UnicodeWidthStr::width`). Compute `prefix_width` by iterating the prefix string's graphemes (`unicode_segmentation::UnicodeSegmentation::graphemes`) and summing `grapheme_width` — NOT a char count.

> **CRITICAL render change (Codex C1):** render builds each row by concatenating spans from screen column 0 — it does NOT position glyphs by `Placed.col`. So offsetting `Placed.col` alone makes the cursor (`source_to_visual` → prefix-inclusive col) misalign with painted text on CONTINUATION rows (which have no prefix glyph span). FIX: render must paint a leading pad of `prefix_width` on EVERY row of a prefixed line — row 0 paints the real prefix glyph (already `prefix_width` wide), and continuation rows (`row_index > 0`) paint a **blank spacer span of `prefix_width` spaces** before the text. So Task 6 spans BOTH layout.rs AND render.rs: add the continuation-row spacer in both the segs and placed paint paths (where `vr.prefix_glyph` is read — paint the glyph on the first visual row, a `" ".repeat(prefix_width)` spacer on later rows of the same logical line). Expose `prefix_width` to render via `ColMap.prefix_width` (the row's map) or store it on `VisualRow`. The wrapped-row test (Step 5) is the gate.

- [ ] **Step 4: Run** `cargo test -p wordcartel-core layout::` — the 3 new tests pass. Then `cargo test -p wordcartel-core` — fix any existing layout test that asserted a `Placed.col` for a PREFIXED line (those now include the offset; update the golden) — non-prefixed lines are unchanged.

- [ ] **Step 4b: Render+cursor wrapped-row gate (the C1 proof).** Add a shell render test: a list item that WRAPS in a narrow area; assert (a) the continuation row's text is painted starting at screen column `text_left + prefix_width` (a blank spacer precedes it — read the cells), and (b) placing the caret on a character of the continuation row yields a `screen_pos` whose col == that char's painted column (cursor on the glyph, not `prefix_width` cells to its left). Mirror an existing wrapped-line render/nav test for setup. This test FAILS if render doesn't pad continuation rows.

- [ ] **Step 5: Run shell cursor/mouse round-trip** `cargo test -p wordcartel nav:: mouse:: render::` — the new wrapped gate passes AND pre-existing nav/mouse tests still pass (non-prefixed lines unchanged). If a test on a list/blockquote line breaks, it was relying on the OLD (buggy) prefix-blind columns — update it to the corrected position (the cursor now lands on the text, not under the bullet).

- [ ] **Step 6: Commit** `feat(theme): cursor-safe prefix geometry — ColMap.prefix_width offsets columns + wrap`

---

## Task 7: Heading-level glyph through layout geometry

**Files:** `wordcartel-core/src/layout.rs` (the `heading_prefix` input); ALL `layout(...)` call sites — `wordcartel/src/derive.rs`, `wordcartel/src/nav.rs`, AND the integration tests that call `layout(...)` directly (`wordcartel-core/tests/block_roles_integration.rs:19`, `wordcartel-core/tests/render_integration.rs:15` — Codex I8; `grep -rn "layout(" wordcartel-core/ wordcartel/` to confirm the full set so the arity change compiles); `wordcartel/src/render.rs` (fill the glyph); test.

**Interfaces:**
- `layout(line, role, is_active, viewport_width, heading_prefix: bool)` — NEW trailing param. When `heading_prefix && role is Heading(_) && !is_active`, reserve a FIXED prefix width (2 cols) and set `prefix_glyph` to a placeholder the render fills (or set `prefix_width` directly + a marker). The glyph CONTENT is theme-driven and filled by render; only the WIDTH lives in layout (so geometry is theme-independent for a given `heading_prefix` setting).
- All `layout(...)` callers pass `heading_prefix = editor.theme.heading_level_glyph` (shell) / `false` (core tests that don't theme).

- [ ] **Step 1: Write the failing tests**
```rust
    #[test]
    fn heading_prefix_reserves_width_when_on() {
        let (_r, on)  = layout("## Title", BlockRole::Heading(2), false, 40, true);
        let (_r, off) = layout("## Title", BlockRole::Heading(2), false, 40, false);
        assert_eq!(on.prefix_width, 2, "heading glyph reserves 2 cols when on");
        assert_eq!(off.prefix_width, 0, "no heading glyph when off");
        // text shifts right by the glyph width when on (cursor-safe)
        assert_eq!(on.source_to_visual(3).1, off.source_to_visual(3).1 + 2);
    }
    #[test]
    fn heading_prefix_off_for_non_heading() {
        let (_r, m) = layout("para", BlockRole::Paragraph, false, 40, true);
        assert_eq!(m.prefix_width, 0);
    }
```

- [ ] **Step 2: Run — fails** (arity / no reservation). `cargo test -p wordcartel-core heading_prefix`

- [ ] **Step 3: Implement.** (a) `layout` gains `heading_prefix: bool`. After computing `prefix_width` from `analysis.prefix_glyph` (Task 6), if `heading_prefix && matches!(role, BlockRole::Heading(_)) && !is_active && analysis.prefix_glyph.is_none()`, set `prefix_width = 2` and set a layout flag/`prefix_glyph = Some("  ".into())` placeholder of width 2 (render replaces the 2-col placeholder with the theme's shade glyph + space). Simplest: set `prefix_glyph = Some("█ ".to_string())` as a DEFAULT heading marker and let render OVERRIDE the glyph char per heading level from the theme — but to keep layout theme-independent, store the LEVEL so render picks the shade: since `VisualRow.role` already carries `Heading(n)`, render can derive the shade char from the level. So: layout just reserves width 2 + emits a 2-col placeholder `prefix_glyph`; render, when `vr.role` is `Heading(n)` and the theme has `heading_level_glyph`, paints the level's shade char + space (`█▓▒░▏·`[n-1]) instead of the placeholder. (b) Update ALL `layout(...)` callers: `wordcartel/src/derive.rs` (the rebuild call — pass `editor.theme.heading_level_glyph`), `wordcartel/src/nav.rs` `layout_line_active`/`layout_line_on_demand` (pass `heading_prefix` from the editor's theme), and any core/integration test callers (`block_roles_integration.rs`, `render_integration.rs`, and any other `layout(` site the grep finds — pass `false`).

- [ ] **Step 4: Render fills the heading glyph.** In render's prefix-glyph paint (segs + placed paths), when `vr.role` is `Heading(n)` and `editor.theme.heading_level_glyph`, paint `["█","▓","▒","░","▏","·"][n.clamp(1,6)-1]` + " " as the prefix (styled via `compose(&theme, depth, &[Heading(n)])`) instead of the layout placeholder. Otherwise paint the layout `prefix_glyph` as before (Task 6 path).

- [ ] **Step 5: Tests.** Core: `cargo test -p wordcartel-core layout:: heading_prefix` — PASS. Shell round-trip: add a render+nav test that a `## Heading` under the No-color theme (heading_level_glyph=true) shows the level glyph AND the cursor on the heading text lands AFTER the glyph (`screen_pos` col == prefix-inclusive). Run `cargo test -p wordcartel` — fix the arity at every `layout(` call (compile-driven).

- [ ] **Step 6: Commit** `feat(theme): heading-level glyph via layout geometry (cursor-safe, theme-driven width)`

---

## Task 8: `typewriter_rows_of_line` prefix-aware

**Files:** `wordcartel/src/nav.rs` (`typewriter_rows_of_line`); test.

**Interfaces:** the shortcut accounts for the row's prefix width (effective capacity = `text_width - prefix_width`).

- [ ] **Step 1: Write the failing test**
```rust
    #[test]
    fn typewriter_rows_prefix_aware() {
        // a list line whose content fits text_width but NOT text_width - prefix_width must wrap.
        let mut ed = crate::editor::Editor::new_from_text("- aaaaa\n", None, (7, 24)); // text_width 7, prefix 2 → cap 5
        ed.view_opts.typewriter = true;
        crate::derive::rebuild(&mut ed);
        // "aaaaa" (5) + prefix 2 = 7 → fits exactly? choose a length that wraps under cap 5.
        // assert the typewriter row count matches the real wrapped rows, not the prefix-blind 1.
        let tw = /* call typewriter_rows_of_line(&ed, 0, 7) via the real path */;
        let real = crate::nav::rows_of_line_pub(&ed, 0); // or compute from the layout cache
        assert_eq!(tw, real);
    }
```
> Adapt to the real (private) fn access — mirror an existing nav test that exercises `typewriter_rows_of_line`/`rows_of_line`. The CONTRACT: for a prefixed line whose raw content fits `text_width` but not `text_width - prefix_width`, the typewriter row count equals the actual wrapped row count (not the shortcut's 1).

- [ ] **Step 2: Run — fails** (shortcut returns 1). 

- [ ] **Step 3: Implement.** In `typewriter_rows_of_line`, fetch the line's prefix width (from the layout cache `line_layouts.get(&li)` → `map.prefix_width`, or recompute), and change the shortcut to `if content_len + prefix_width <= text_width { 1 } else { rows_of_line(editor, li) }`. (Get `prefix_width` from the cached `ColMap` if present, else fall through to `rows_of_line`.)

- [ ] **Step 4: Run** `cargo test -p wordcartel nav::` — PASS, no regression on non-prefixed lines.

- [ ] **Step 5: Commit** `fix(theme): typewriter_rows_of_line accounts for prefix width`

---

## Task 9: Document-selection painting

**Files:** `wordcartel-core/src/theme.rs` (`default()` selection face — Codex I5); `wordcartel/src/render.rs` (placed-path + segs-path glyph loops); `editor.rs`/test.

**Interfaces:** Consumes `editor.active().document.selection.primary()` (`from()`/`to()`/`is_empty()`); layers the `Selection` face (reverse) on glyphs inside the primary range, in BOTH render paths; a non-empty selection forces the placed path.

- [ ] **Step 1: Write the failing tests**
```rust
    #[test]
    fn selection_paints_reverse_on_selected_cells() {
        let mut ed = Editor::new_from_text("hello world\n", None, (40, 4));
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5); // "hello"
        let buf = render_to_buffer(&mut ed, 40, 4);
        // a selected cell (col 0..5 on row 0) carries the Selection face; compose([..,Selection]) under Default
        let sel = compose::compose(&ed.theme, ed.depth, &[wordcartel_core::theme::SemanticElement::Selection]);
        assert!((0..5).any(|x| buf[(x,0)].style().add_modifier.contains(Modifier::REVERSED)));
        let _ = sel;
    }
    #[test]
    fn empty_selection_paints_nothing() {
        let mut ed = Editor::new_from_text("hello\n", None, (40, 4));
        ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
        let buf = render_to_buffer(&mut ed, 40, 4);
        assert!(!(0..5).any(|x| buf[(x,0)].style().add_modifier.contains(Modifier::REVERSED)));
    }
```

- [ ] **Step 2: Run — fails** (selection not painted, AND Default's selection face is empty). 

- [ ] **Step 3a: Give the Default theme a visible selection face (Codex I5).** Plan ① left `default()`'s `selection: Face::default()` (empty) because ① didn't paint selection. Now that ② paints it, the Default theme needs a cue or `compose([Selection])` is a no-op and the Step-1 test fails. In `theme.rs::default()`, set `selection: Face { reverse: Some(true), ..Face::default() },`. (no_color/phosphor already carry reverse via `mono_faces`.) If a plan-① theme test pins `default().selection == Face::default()`, update it to expect the reverse face.

- [ ] **Step 3: Implement (render).** Before the row loop, snapshot the primary selection: `let sel = editor.active().document.selection.primary(); let (sel_from, sel_to) = (sel.from(), sel.to()); let has_sel = !sel.is_empty();`. Force the placed path when `has_sel` (mirror how search forces it — find the `use_placed` flag and OR in `has_sel`). In the placed-path per-glyph loop, after the search patch and BEFORE the diagnostic, add:
```rust
                    let is_selected = has_sel && overlaps(g_from, g_to, sel_from, sel_to);
                    if is_selected {
                        let sel_face = editor.theme.face(SE::Selection);
                        style = style.patch(crate::compose::face_to_ratatui(&sel_face, editor.depth));
                    }
```
(`overlaps` + `g_from`/`g_to` already exist in the loop.) Selection paints in BOTH live-preview and source modes (the loop runs in both); it's below search so a search-current match still stands out.

- [ ] **Step 4: Run** `cargo test -p wordcartel render:: selection_paints empty_selection` + full `cargo test -p wordcartel` — PASS, no regression.

- [ ] **Step 5: Commit** `feat(theme): paint the document selection (Selection face, both render paths)`

---

## Task 10: §13.2 accessibility coverage proof

**Files:** `wordcartel/src/render.rs` (test module); test only.

**Interfaces:** a render-level proof that EVERY `SemanticElement` is distinguishable by modifier/glyph in cue mode (No-color AND a phosphor-flat), + pairwise collision tests for the same-context persistent pairs.

- [ ] **Step 1: Write the proof tests** (no production change — these LOCK §13.2):
```rust
    fn cue_themes() -> [wordcartel_core::theme::Theme; 2] {
        [wordcartel_core::theme::no_color(),
         wordcartel_core::theme::Theme::builtin("phosphor-amber-flat").unwrap()]
    }
    #[test]
    fn a11y_every_cued_element_has_a_modifier_in_cue_mode() {
        use wordcartel_core::theme::SemanticElement::*;
        // the Face-cued elements (glyph-cued ones are proven by the render fixtures below)
        let cued = [Emphasis, Strong, StrongEmphasis, Code, CodeBlock, Link, Strikethrough,
                    Comment, FrontMatter, Selection, SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar];
        for t in cue_themes() {
            for el in cued {
                let f = t.face(el);
                assert!(f.bold.unwrap_or(false)||f.italic.unwrap_or(false)||f.underline.unwrap_or(false)
                        ||f.strike.unwrap_or(false)||f.reverse.unwrap_or(false)||f.dim.unwrap_or(false),
                        "{}/{el:?} needs a non-color cue", t.name);
            }
        }
    }
    #[test]
    fn a11y_pairwise_distinct_same_context_pairs() {
        use wordcartel_core::theme::SemanticElement::*;
        for t in cue_themes() {
            assert_ne!(t.face(Comment), t.face(Emphasis), "{}: Comment vs Emphasis", t.name);
            assert_ne!(t.face(FrontMatter), t.face(Code), "{}: FrontMatter vs Code", t.name);
            assert_ne!(t.face(Selection), t.face(Code), "{}: Selection vs Code", t.name);
            // spelling vs grammar are different underline COLORS today; in cue mode they
            // must stay distinguishable by modifier (Codex I7 — §13.2 is fully closed).
            assert_ne!(t.face(DiagSpelling), t.face(DiagGrammar), "{}: DiagSpelling vs DiagGrammar", t.name);
        }
    }
    #[test]
    fn a11y_structural_glyphs_render_in_no_color() {
        // blockquote ▎, thematic-break ───, heading shade glyph all PAINT under No-color (glyph cue).
        let mut ed = Editor::new_from_text("> quote\n\n---\n\n### H3\n", None, (40, 12));
        ed.theme = wordcartel_core::theme::no_color(); // heading_level_glyph = true
        crate::derive::rebuild(&mut ed);
        let text = (0..12).map(|r| row_string(&render_to_buffer(&mut ed, 40, 12), r)).collect::<String>();
        assert!(text.contains('▎'), "blockquote bar");
        assert!(text.contains('─'), "thematic rule");
        assert!(text.contains('▒') || text.contains('█'), "heading shade glyph"); // h3 = ▒
    }
```

- [ ] **Step 1b: Make spelling/grammar cues distinct in cue mode (Codex I7).** `mono_faces()` (theme.rs, plan ①) currently gives BOTH `DiagSpelling` and `DiagGrammar` bold+underline — identical, so the new pairwise assert fails and the spelling-vs-grammar distinction (different underline colors today) is lost without color. Change one cue so they differ while both stay underlined (both are diagnostics): e.g. `DiagSpelling` = bold+underline, `DiagGrammar` = italic+underline. Update `mono_faces` accordingly. If a plan-① `no_color`/phosphor test pins `DiagGrammar`'s exact face, update it to the new cue.

- [ ] **Step 2: Run** `cargo test -p wordcartel a11y_` — these assert CURRENT behavior after Tasks 1-9 + Step 1b; they should PASS. If `a11y_structural_glyphs_render_in_no_color` fails, a glyph isn't reaching the buffer — that's a real §13.2 gap to fix in the relevant producer/render task, not a test to weaken.

- [ ] **Step 3: Run the FULL workspace** `XDG_STATE_HOME=/tmp/wc-theme2 cargo test -p wordcartel && cargo test -p wordcartel-core` — all green.

- [ ] **Step 4: Commit** `test(theme): §13.2 coverage proof (cue-mode modifiers + glyphs + pairwise distinct)`

---

## Final Verification
- [ ] `cargo test` (workspace) — all green.
- [ ] `cargo clippy -p wordcartel-core -p wordcartel --lib` — no NEW warnings in the touched files.
- [ ] Manual smoke: a doc with front matter + a blockquote + a `<!-- comment -->` + a list + headings; under No-color confirm ▎/───/heading-glyphs render and the cursor lands correctly on prefixed lines (click a list bullet's text — caret lands on the char, not under the bullet); select text and see reverse; under phosphor-amber confirm the whole thing tints amber.

## Self-Review Notes (coverage vs spec §12 plan ②)
- §3.9 producers → Tasks 2 (inline comment), 3 (block comment), 4 (front matter), + Style::Comment (1).
- §4 structural glyphs → Task 5 (blockquote/hr) + Task 7 (heading-level glyph).
- §3.7 cursor-safe prefix geometry → Task 6 (ColMap.prefix_width keystone) + Task 7 (heading width) + Task 8 (typewriter).
- §3.4/§3.5 selection painting → Task 9.
- §4/§8.3 §13.2 proof → Task 10.
- **Deferred to plan ③ (correctly NOT here):** base16 import, `[theme]` config, depth detection, the theme picker + relayout-on-switch wiring. Plan ② consumes the theme already seeded on `Editor` (plan ①).
