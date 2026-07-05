# C2 Transform Scope Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** empty-selection transforms act on the transform unit under the caret (never the whole buffer); selection snapping goes endpoint-deepest via the same unit lookup; explicit `reflow_buffer`/`unwrap_buffer`/`ventilate_buffer` commands carry whole-document intent.

**Architecture:** a new `transform_unit_at(text, blocks, pos)` in transform.rs implements the spec's full unit rule (nearest-ListItem-else-BlockQuote-else-leaf, line-blank gaps, the line-keyed N5 refinement, line-start extension) reusing core's public `TextSource`; `snap_to_blocks` becomes two endpoint unit lookups; `dispatch_transform` gains one `Option<Range<usize>>` region parameter so both existing guards cover both scopes. Three tasks: T1 the lookup + snapping (transform.rs only), T2 the `_buffer` variants + dispatch migration, T3 behavior pins + e2e.

**Tech Stack:** Rust; shell crate only; no new dependencies; no core changes (`TextSource`/`Block` are already public).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-04-wordcartel-c2-transform-scope-design.md` (CLEAN — Codex ×4 + Fable ×4; three user ratifications: the two 2026-07-03 decisions + N5-A). Its D1-D4 rules and probe-verified span anatomy govern.
- **Gates after EVERY commit:** `cargo test -p wordcartel-core -p wordcartel` green, `cargo clippy --workspace --all-targets` clean (deny gate LIVE), `cargo build` warning-free. NO `cargo fmt`; `—` em-dash prose; hand-match neighbors.
- **Probe-robust test discipline:** unit tests build trees via the existing `blocks_of(text)` helper (transform.rs:219-221) and assert against THE TREE'S OWN spans (`bt.top_level()[i].children[j].span`) wherever possible — hardcoded byte offsets only where the grounding's corpus tables pin them, and every `[verify]`-marked offset MUST be confirmed against the parsed tree in the test itself (assert the precondition, e.g. `assert_eq!(item.span.start, 10)`), not assumed.
- The four `snap_*` tests update per the spec's enumeration (mid-paragraph and fence keep their spans; multi-block rephrases; nested cases are new). NO other existing test changes meaning; the three app.rs transform tests survive T1 unchanged (single-block corpora — spec/Fable M6).
- Line anchors are HEAD (`67aec16`) references; locate by quoted code after earlier tasks shift lines.
- Every commit message ends with the trailers, verbatim (use `git commit -F -` with a quoted heredoc — `!` breaks zsh inside double-quoted `-m`):
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: `transform_unit_at` + endpoint snapping (transform.rs only)

**Files:**
- Modify: `wordcartel/src/transform.rs` (the new lookup, `snap_to_blocks`, `region_for_transform`, the four snap-test updates, the unit-lookup test battery)

**Interfaces:**
- Consumes: `wordcartel_core::block_tree::{BlockTree, Block, BlockKind, TextSource}` (all public); `doc.buffer.snapshot() -> ropey::Rope` (`&Rope: TextSource`); `doc.blocks() -> &BlockTree` (cheap borrow).
- Produces: `fn transform_unit_at(text: impl TextSource + Copy, blocks: &BlockTree, pos: usize) -> Option<std::ops::Range<usize>>` (private) and the re-signatured `pub fn snap_to_blocks(text: impl TextSource + Copy, blocks: &BlockTree, from: usize, to: usize) -> Range<usize>` — Task 2's dispatch changes ride on `region_for_transform`, whose signature is UNCHANGED.

- [ ] **Step 1: the failing unit-lookup tests.** In transform.rs's test module, add (names from the spec; bodies follow the `blocks_of` + tree-derived-span discipline — the grounding's §B tables give the corpora and expected arithmetic; `[verify]`-marked offsets get precondition asserts):

```rust
    #[test]
    fn caret_region_is_the_transform_unit() {
        // Item body → the ITEM span (marker included), line-start-extended (§B.1:
        // inner item span 14..84 excludes the 2-space indent; the unit is 12..84).
        let text = "- outer one\n  - inner one two three four five six seven\n    continuation words here\n";
        let bt = blocks_of(text);
        let outer = &bt.top_level()[0].children[0];            // outer ListItem
        let inner_list = outer.children.iter().find(|c| matches!(c.kind, wordcartel_core::block_tree::BlockKind::List)).unwrap();
        let inner = &inner_list.children[0];                    // inner ListItem
        assert_eq!(inner.span.start, 14, "precondition: item span excludes indent");
        let u = transform_unit_at(text, &bt, 20).expect("caret in inner body");
        assert_eq!(u, 12..inner.span.end, "nearest ListItem, extended to line start");
        // Plain paragraph → its own leaf span.
        let text2 = "para one here\n\npara two here\n";
        let bt2 = blocks_of(text2);
        let p0 = bt2.top_level()[0].span.clone();
        assert_eq!(transform_unit_at(text2, &bt2, 3), Some(p0));
    }

    #[test]
    fn transform_unit_in_item_body_is_the_item_not_the_paragraph() {
        // Loose item: the body sits in a Paragraph CHILD (2..13, probe) — the unit must be
        // the ITEM (0..14, trailing blank included per anatomy), never the paragraph.
        let text = "- alpha item\n\n- beta item\n";
        let bt = blocks_of(text);
        let item1 = &bt.top_level()[0].children[0];
        assert!(item1.children.iter().any(|c| matches!(c.kind, wordcartel_core::block_tree::BlockKind::Paragraph)),
            "precondition: loose item wraps its text in a Paragraph");
        let u = transform_unit_at(text, &bt, 5).expect("caret in alpha body");
        assert_eq!(u, item1.span.clone(), "the item, marker and trailing blank included");
    }

    #[test]
    fn transform_unit_in_nested_item_is_the_deepest_item() {
        let text = "- outer one\n  - inner one two three four five six seven\n    continuation words here\n";
        let bt = blocks_of(text);
        let u = transform_unit_at(text, &bt, 60).expect("caret in the continuation line");
        assert!(u.start == 12, "the INNER item (line-start-extended), not the outer: {u:?}");
    }

    #[test]
    fn caret_region_in_gap_is_none_and_blankline_gaps() {
        // Top-level gap (blank between paragraphs) → None.
        let text = "para one\n\npara two\n";
        let bt = blocks_of(text);
        assert_eq!(transform_unit_at(text, &bt, 9), None, "top-level blank");
        // Loose-list trailing blank (byte 13 — INSIDE item 1's span per anatomy) →
        // container descent + blank line → None.
        let text2 = "- alpha item\n\n- beta item\n";
        let bt2 = blocks_of(text2);
        assert_eq!(transform_unit_at(text2, &bt2, 13), None, "loose blank is a gap");
    }

    #[test]
    fn caret_on_loose_item_marker_transforms_the_item() {
        // Marker bytes (0..2) are container-interior on a NON-blank line → the item.
        let text = "- alpha item\n\n- beta item\n";
        let bt = blocks_of(text);
        let item1 = bt.top_level()[0].children[0].span.clone();
        assert_eq!(transform_unit_at(text, &bt, 0), Some(item1));
    }

    #[test]
    fn caret_in_tight_item_lead_text_transforms_the_item() {
        // Tight item lead text (bytes 2..11) has NO Paragraph child → outer item 0..84.
        let text = "- outer one\n  - inner one two three four five six seven\n    continuation words here\n";
        let bt = blocks_of(text);
        let outer = bt.top_level()[0].children[0].span.clone();
        assert_eq!(transform_unit_at(text, &bt, 5), Some(outer));
    }

    #[test]
    fn caret_in_nested_item_indent_transforms_the_child_item() {
        // The N5 line-keyed refinement, all three ratified shapes:
        // (a) first-nested indent (§B.5: bytes 8-9 → inner item 10..18 → unit 8..18);
        let text = "- outer\n  - inner\n";
        let bt = blocks_of(text);
        let inner_first = &bt.top_level()[0].children[0]
            .children.iter().find(|c| matches!(c.kind, wordcartel_core::block_tree::BlockKind::List))
            .and_then(|l| l.children.first()).expect("nested item").span;
        assert_eq!(*inner_first, 10..18, "precondition (probe-verified)");
        let u = transform_unit_at(text, &bt, 8).expect("indent byte");
        assert_eq!(u, 8..18, "the INNER item, line-start-extended");
        // (b) space-indented TOP-LEVEL item " - a" (precondition-assert it parses as a list);
        let text2 = " - a\n";
        let bt2 = blocks_of(text2);
        assert!(matches!(bt2.top_level()[0].kind, wordcartel_core::block_tree::BlockKind::List),
            "precondition: 1-space indent still a list");
        let item = bt2.top_level()[0].children[0].span.clone();
        assert_eq!(transform_unit_at(text2, &bt2, 0), Some(0..item.end));
        // (c) TAB-indented NESTED item — probe-verified corpus (Fable plan C1: for
        // "- x\n\t- a\n" pulldown starts the inner span at the PREVIOUS newline —
        // mid-tab, unsplittable — so THAT shape degrades; use the ordered-outer form
        // whose inner span starts on the tab line).
        let text3 = "1. x\n\t- a\n";
        let bt3 = blocks_of(text3);
        let outer3 = &bt3.top_level()[0].children[0];
        let inner3 = outer3.children.iter().find(|c| matches!(c.kind, wordcartel_core::block_tree::BlockKind::List))
            .and_then(|l| l.children.first()).expect("nested item exists");
        assert_eq!(inner3.span.start, 5, "precondition: inner span starts ON the tab line");
        let u3 = transform_unit_at(text3, &bt3, 5).expect("tab indent byte");
        assert_eq!(u3, 5..inner3.span.end, "the nested item, line-start-extended");
        // (d) The DEGRADATION pin (user-ratified 2026-07-05): the mid-tab shape's
        // inner span starts at the previous newline → the unit is the OUTER item.
        let text4 = "- x\n\t- a\n";
        let bt4 = blocks_of(text4);
        let outer4 = bt4.top_level()[0].children[0].span.clone();
        let u4 = transform_unit_at(text4, &bt4, 4).expect("tab byte");
        assert_eq!(u4, 0..outer4.end, "mid-tab span shape degrades to the outer item — accepted");
    }

    #[test]
    fn caret_in_quote_body_is_the_blockquote_and_item_beats_quote() {
        // Bare quote → the BlockQuote span (a bare Paragraph slice mixes "> " prefixes).
        let text = "> quoted line one\n> quoted line two\n";
        let bt = blocks_of(text);
        let q = bt.top_level()[0].span.clone();
        assert_eq!(transform_unit_at(text, &bt, 5), Some(q));
        // Quote nested in an item → the ITEM wins (ListItem beats BlockQuote).
        let text2 = "- outer\n  > quoted line\n";
        let bt2 = blocks_of(text2);
        let item = bt2.top_level()[0].children[0].span.clone();
        let u2 = transform_unit_at(text2, &bt2, 14).expect("caret in nested quote body");
        assert_eq!(u2.end, item.end, "the outer item's unit, not the inner quote");
    }

    #[test]
    fn degraded_parse_caret_is_nothing_to_transform() {
        // Fable plan C2: a childless Document root (the M4-rest panic fallback's
        // empty_tree shape) must yield None — never the whole buffer. Build the
        // degenerate tree by hand (Block fields are pub).
        let text = "some text here ok\n";
        let bt = wordcartel_core::block_tree::BlockTree {
            root: wordcartel_core::block_tree::Block {
                kind: wordcartel_core::block_tree::BlockKind::Document,
                span: 0..text.len(),
                children: Vec::new(),
            },
        };
        assert_eq!(transform_unit_at(text, &bt, 5), None, "degraded parse: no unit, the guard says nothing-to-transform");
    }

    #[test]
    fn caret_region_at_end_of_buffer_clamps() {
        let text = "only para\n";
        let bt = blocks_of(text);
        // region_for_transform clamps caret==buf_len to the last byte.
        let mut e = Editor::new_from_text(text, None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(text.len());
        let r = region_for_transform(&e.active().document);
        assert_eq!(r, bt.top_level()[0].span.clone());
    }
```

Run: `cargo test -p wordcartel -- transform_unit caret_` (multiple filters OR-match after `--`; Fable plan M1) — FAIL to compile (`transform_unit_at` doesn't exist): the recorded RED.

- [ ] **Step 2: implement the lookup.** In transform.rs, above `snap_to_blocks`, add (complete):

```rust
use wordcartel_core::block_tree::TextSource;

/// Extend a unit span to the start of its first line — nested-item and quote spans
/// exclude their leading indent/prefix bytes, and a mid-line slice makes repar
/// reflow at the wrong content column (spec C2 D1, Fable r5 C1).
fn extend_to_line_start(text: impl TextSource, span: std::ops::Range<usize>) -> std::ops::Range<usize> {
    text.line_start(span.start)..span.end
}

/// The deepest ListItem beneath `node` whose span STARTS exactly at `at` — the
/// N5 line-keyed refinement's target (the line's first non-whitespace content
/// begins this item; List spans start at the same byte, so descend through them).
fn item_starting_at(node: &wordcartel_core::block_tree::Block, at: usize) -> Option<&wordcartel_core::block_tree::Block> {
    for c in &node.children {
        if at >= c.span.start && at < c.span.end {
            if matches!(c.kind, wordcartel_core::block_tree::BlockKind::ListItem) && c.span.start == at {
                return Some(c);
            }
            if let Some(f) = item_starting_at(c, at) {
                return Some(f);
            }
        }
    }
    None
}

/// The transform UNIT enclosing `pos` (spec C2 D1): the nearest ListItem on the
/// descent path (marker included), else the nearest BlockQuote, else the deepest
/// leaf. None when `pos` sits on a blank line inside a container (a gap — never
/// snap structural blanks to whole containers) or matches nothing. Non-blank
/// container-interior bytes resolve via the same preference set, with the N5
/// refinement: when the byte's line's first non-whitespace content begins a
/// ListItem at any depth beneath the descent's final node, THAT item is the unit
/// — Home on a nested item's line transforms the item the eye is on. Every
/// returned span is extended to its line start.
fn transform_unit_at(
    text: impl TextSource + Copy,
    blocks: &wordcartel_core::block_tree::BlockTree,
    pos: usize,
) -> Option<std::ops::Range<usize>> {
    let mut path: Vec<&wordcartel_core::block_tree::Block> = vec![&blocks.root];
    loop {
        let node = *path.last().expect("path is never empty");
        match node.children.iter().find(|c| pos >= c.span.start && pos < c.span.end) {
            Some(c) => path.push(c),
            None => break,
        }
    }
    let last = *path.last().expect("path is never empty");
    // The ROOT is never a leaf: the degraded-parse fallback (empty_tree,
    // block_tree.rs:333-335) yields a childless Document root — treating it as a
    // leaf would return 0..len, the whole-buffer transform this effort kills
    // (Fable plan C2; the container branch correctly yields None instead).
    let in_leaf = path.len() > 1
        && last.children.is_empty()
        && pos >= last.span.start
        && pos < last.span.end;
    let nearest = |kind_test: fn(&wordcartel_core::block_tree::BlockKind) -> bool| {
        path.iter().rev().find(|b| kind_test(&b.kind)).map(|b| b.span.clone())
    };
    if in_leaf {
        let unit = nearest(|k| matches!(k, wordcartel_core::block_tree::BlockKind::ListItem))
            .or_else(|| nearest(|k| matches!(k, wordcartel_core::block_tree::BlockKind::BlockQuote)))
            .unwrap_or_else(|| last.span.clone());
        return Some(extend_to_line_start(text, unit));
    }
    // Container-interior (or unmatched) byte: discriminate by line blankness.
    let ls = text.line_start(pos);
    let le = text.line_end(pos);
    let line = text.slice(ls..le);
    if line.trim().is_empty() {
        return None; // structural blank = gap, regardless of ancestors (spec r3/r5)
    }
    // N5 line-keyed refinement (user-ratified A, r7 P1 wording).
    let first_content = ls + (line.len() - line.trim_start().len());
    if let Some(item) = item_starting_at(last, first_content) {
        return Some(extend_to_line_start(text, item.span.clone()));
    }
    nearest(|k| matches!(k, wordcartel_core::block_tree::BlockKind::ListItem))
        .or_else(|| nearest(|k| matches!(k, wordcartel_core::block_tree::BlockKind::BlockQuote)))
        .map(|span| extend_to_line_start(text, span))
}
```

- [ ] **Step 3: re-signature `snap_to_blocks` + rewire `region_for_transform`.** Replace both (:62-80, :84-92) with (complete; the doc comments updated to the new semantics):

```rust
/// Snap a non-empty selection's ENDPOINTS to their transform units (spec C2 D2):
/// start = the unit at `from` (its extended start), end = the unit at the last
/// selected byte (its end); gap endpoints stay raw. The interior rides between.
pub fn snap_to_blocks(
    text: impl TextSource + Copy,
    blocks: &wordcartel_core::block_tree::BlockTree,
    from: usize,
    to: usize,
) -> std::ops::Range<usize> {
    let start = transform_unit_at(text, blocks, from).map(|u| u.start).unwrap_or(from);
    let end = transform_unit_at(text, blocks, to.saturating_sub(1)).map(|u| u.end).unwrap_or(to);
    if start < end { start..end } else { from..to }
}

/// The byte range a transform should reformat: the transform unit under the
/// caret when the primary selection is empty (an empty range on a gap — the
/// dispatch guard turns that into "nothing to transform"), else the selection
/// endpoint-snapped to whole units.
pub fn region_for_transform(doc: &crate::editor::Document) -> std::ops::Range<usize> {
    let sel = doc.selection.primary();
    let buf_len = doc.buffer.len();
    let snapshot = doc.buffer.snapshot();
    if sel.is_empty() {
        let caret = sel.from().min(buf_len.saturating_sub(1));
        transform_unit_at(&snapshot, doc.blocks(), caret).unwrap_or(sel.from()..sel.from())
    } else {
        snap_to_blocks(&snapshot, doc.blocks(), sel.from(), sel.to())
    }
}
```

(`&Rope: TextSource + Copy` — the snapshot binding outlives both calls. `buf_len == 0` → caret 0, no block → `0..0` → the guard.)

- [ ] **Step 4: update the four snap tests + add the snapping pins.** The existing four (:230-281) gain the `text` argument (`snap_to_blocks(text, &bt, …)`); mid-paragraph, fence, and past-EOF keep their assertions VERBATIM (leaf top-level blocks — same spans under unit semantics); multi-block keeps its assertion (paragraph endpoints are leaves; the union is identical). Add the new snapping pins per the spec (bodies in the Step-1 discipline; corpora from grounding §B):
  - `snap_inside_one_list_item_touches_only_that_item` (three-item tight list, §B.6 — selection inside item 2 → exactly item 2's line-start-extended span; precondition-assert the item spans against the tree);
  - `snap_across_three_items_touches_exactly_those` (five-item list, select from inside item 1 to inside item 3 → items 1-3);
  - `snap_paragraph_into_list_unions_endpoints` (paragraph then list; select from para into item 1 → para start..item 1 end);
  - `snap_selection_wholly_in_gap_returns_input` (both endpoints on blanks → `from..to`);
  - `snap_endpoint_on_loose_list_blank_is_gap_not_container` (§B.2: from=5, to=14 → `0..14`, item 2 untouched);
  - `snap_endpoint_on_nested_list_interitem_blank_is_gap` (§B.4 corpus `"- outer\n  - a\n\n  - b\n"` — precondition-assert the blank byte's location from the tree, endpoint there stays raw, the outer item is NOT pulled in).

- [ ] **Step 5: GREEN + the three survivors.** Full transform.rs suite green; then verify the three app.rs transform tests pass UNCHANGED (single-block corpora — the caret default reaches the same region; record in the report). Full gates.

- [ ] **Step 6: commit** — `feat(c2): transform_unit_at — caret acts on the unit under it; endpoint-deepest snapping`.

---

### Task 2: the `_buffer` variants + dispatch migration

**Files:**
- Modify: `wordcartel/src/transform.rs` (`dispatch_transform` + guard tests), `wordcartel/src/registry.rs` (three registrations + test), `wordcartel/src/prompts.rs` (one call site), `wordcartel/src/app.rs` (three test call sites + the rehomed/new tests)

**Interfaces:**
- Consumes: Task 1's semantics (no signatures from it).
- Produces: `dispatch_transform(editor, kind, region: Option<Range<usize>>, clock, msg_tx)` — `None` = caret/selection default; `Some(0..len)` = the `_buffer` scope. Task 3 consumes the six commands.

- [ ] **Step 1: the signature + call-site migration.** Per the grounding's smallest-diff shape: add the `region: Option<std::ops::Range<usize>>` parameter THIRD; the body's only change is
  `let range = region.unwrap_or_else(|| region_for_transform(&editor.active().document));`
  replacing the direct call — both guards stay exactly where they are (the in-flight guard FIRST, then the region resolution, then the empty guard; Fable I3's requirement). Migrate all seven call sites (prompts.rs:209, registry.rs:286/:290/:294, app.rs:2706/:2723/:2774) to pass `None`, and update the stale signature comment at app.rs:2705 ("dispatch_transform takes (editor, kind, clock, msg_tx)" — Fable plan M5).

- [ ] **Step 2: the three `_buffer` registrations** (registry.rs, immediately after `ventilate`'s):

```rust
        r.register("reflow_buffer", "Reflow Buffer", Some(MenuCategory::Format), |c| {
            let len = c.editor.active().document.buffer.len();
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Reflow, Some(0..len), c.clock, &c.msg_tx);
            CommandResult::Handled
        });
        r.register("unwrap_buffer", "Unwrap Buffer", Some(MenuCategory::Format), |c| {
            let len = c.editor.active().document.buffer.len();
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Unwrap, Some(0..len), c.clock, &c.msg_tx);
            CommandResult::Handled
        });
        r.register("ventilate_buffer", "Ventilate Buffer", Some(MenuCategory::Format), |c| {
            let len = c.editor.active().document.buffer.len();
            crate::transform::dispatch_transform(c.editor, crate::transform::TransformKind::Ventilate, Some(0..len), c.clock, &c.msg_tx);
            CommandResult::Handled
        });
```

  Extend `transforms_are_registered_commands_in_format_category` (registry.rs:573) to the six-entry array (grounding §C.3).

- [ ] **Step 3: the guard pins + the rehomed tests** (app.rs test module, the §C.2 idioms):
  - `buffer_variant_rejected_while_in_flight`: set `e.transform_in_flight = true`, dispatch with `Some(0..len)` → status "a transform is already running", nothing else happens.
  - `buffer_variant_on_empty_buffer_says_nothing_to_transform`: empty buffer (`""` → len 0 → `Some(0..0)`) → status "nothing to transform".
  - `reflow_whole_buffer_applies_one_undoable_edit` → REHOME as `reflow_buffer_applies_one_undoable_edit`: same corpus and assertions, dispatched with `Some(0..len)` (the variant's scope).
  - NEW `caret_reflow_acts_on_caret_block_only` (the multi-block sibling — spec/Fable M6): TWO long paragraphs; caret in paragraph 1 (`Selection::single(5)`); dispatch `None` → paragraph 1 rewrapped, paragraph 2 byte-identical, one undo restores.
  - NEW `reflow_buffer_routes_async_on_giant_buffer`: the `"word ".repeat(300_000)` corpus dispatched with `Some(0..len)` → `transform_in_flight` true, `TransformDone` arrives (the sibling of the UNCHANGED caret-default async test).
  - `caret_reflow_on_blank_line_noops_with_status`: two paragraphs with a blank between; caret ON the blank; dispatch `None` → status "nothing to transform", buffer + version unchanged.
  - `caret_reflow_in_fence_noops`: a fenced block with a long code line; caret inside; dispatch `None` (Reflow) → buffer unchanged, status contains "already".

- [ ] **Step 4: GREEN + full gates.**

- [ ] **Step 5: commit** — `feat(c2): explicit _buffer transform commands; dispatch takes an explicit region behind the shared guards`.

---

### Task 3: behavior pins + e2e journey

**Files:**
- Modify: `wordcartel/src/app.rs` (two behavior pins), `wordcartel/src/e2e.rs` (the journey)

**Interfaces:** consumes Tasks 1-2; produces the shipped user-visible pins.

- [ ] **Step 1: the sibling-preservation pins** (app.rs, §C.2 idioms; corpora from §B with precondition asserts):
  - `caret_reflow_inside_item_preserves_siblings`: the three-item tight list (§B.6) with item 2's text long enough to actually rewrap at width 72 (lengthen item 2's words accordingly); caret in item 2; dispatch `None` (Reflow) → items 1 and 3 byte-identical (compare the exact prefix/suffix slices), item 2 still begins `- `, one undo restores.
  - `caret_reflow_inside_nested_item_preserves_indent` (**Fable r5 C1's behavior pin**): the §B.1 corpus with the inner item lengthened past width 72; caret in the inner body; dispatch `None` → line 1 (`- outer one`) byte-identical; the changed region still begins `  - ` (2-space indent + marker); every continuation line begins with exactly 4 spaces; one undo restores. (Assert the marker/indent INVARIANTS, not the exact wrap — grounding §B.1.)

- [ ] **Step 2: the e2e journey** (e2e.rs, §C.4 idioms):
  - `journey_transform_scopes`: build a three-item list doc; `Selection::single` into item 2; `h.ctrl('t')` → chooser visible (`screen_contains("[r]eflow")` or the chooser's real prompt text — read `Prompt::transform_chooser`'s message and use it); type `r` → only item 2's text changes in `h.doc_text()` (items 1/3 substrings still present verbatim); then `h.ctrl('p')`, `h.type_str("reflow buffer")` (precondition: filters to the `Reflow Buffer` row), Enter → the whole document rewraps (`h.doc_text()` differs in items 1/3 too, if they were long enough — make all three items long).

- [ ] **Step 3: full gates + smoke.** Run `scripts/smoke/run.sh` once and QUOTE the one-line summary verbatim in the report (advisory).

- [ ] **Step 4: commit** — `feat(c2): transform-scope behavior pins + e2e journey`.

---

## Verification appendix (final whole-branch review charge)

- The three ratified decisions hold: caret default = the unit (never the buffer); deepest snapping incl. all Fable-round semantics (line-blank gaps, line-keyed N5, line-start extension, ListItem-beats-BlockQuote); `_buffer` variants explicit with shared guards.
- The four snap tests updated as enumerated; the three app.rs transform tests' fates exactly as the spec states (two survive unchanged, one rehomed with the multi-block sibling added); the chooser tests and `ctrl-t` untouched.
- The fragment landmine unreachable: every unit is line-start-extended and marker-inclusive (the nested-item behavior pin proves the round trip).
- The `[verify]`-marked corpus facts were precondition-asserted in tests, not assumed (tab-nested shape, loose-item Paragraph child, quote spans).
- Flagged spec-name/home deviations (Fable plan M3, recorded): `caret_region_in_gap_is_empty` → `caret_region_in_gap_is_none_and_blankline_gaps`; the loose-marker pin homed as a transform.rs unit test; the rehomed reflow_buffer test dispatches `Some(0..len)` directly — the registered closures' `0..len` construction is exercised by the T3 palette journey.
- `deepest_block_at`/`paragraph_range_at` (nav.rs) untouched; no core changes; no new deps; no `#[allow]`; no `unsafe`.
- Pre-merge: smoke verbatim + a live tmux sanity (three-item list, caret in item 2, ctrl-t r → one item changes; palette Reflow Buffer → all change).
- Controller merge-time bookkeeping: backlog C2 → SHIPPED (the gap-caret no-op convention, chooser-means-block, the N5 refinement); working order advances to D1+A5.
