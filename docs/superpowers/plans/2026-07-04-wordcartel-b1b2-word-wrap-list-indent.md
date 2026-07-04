# B1+B2 Word-Boundary Wrap + Nested-List Indent Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** UAX #14 word-boundary soft-wrap (with CodeBlock exemption, trailing-whitespace hang, grapheme fallback) + nested-list indent conceal with automatic hanging indent — one document-text-rendering effort.

**Architecture:** a pure break-opportunity helper in `wordcartel-core/src/layout.rs` (new dep `unicode-linebreak`), a reworked wrap loop that re-places the post-break tail, a tab-aware marker-conditional ListItem conceal in `md_parse.rs`, and a render-side caret clamp. Laws: Law 3 amended composably; new W1/W2. All geometry consumers are verified untouched by the spec.

**Tech Stack:** Rust; `unicode-linebreak = "0.1"` (Unicode 15.0.0, no deps, no unsafe, `#![no_std]` — compatible with core's `#![forbid(unsafe_code)]`).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-04-wordcartel-b1b2-word-wrap-list-indent-design.md` (CLEAN — Codex ×3 + Fable ×2, empirically probed). Its D1-D6 rules and law statements govern; quote them when in doubt.
- **Gates after EVERY commit:** `cargo test -p wordcartel-core -p wordcartel` green, `cargo clippy --workspace --all-targets` clean (deny gate LIVE), `cargo build` warning-free. NO `cargo fmt`; house style: `—` em-dash in prose comments, no emoji outside multibyte-corpus tests, hand-wrapped ~100-char lines matching neighbors.
- Never weaken a law or an existing test's property — updates change EXPECTATIONS to the new geometry only where the spec's enumeration says so; tests listed as "unchanged" must pass unmodified (they become fallback/geometry pins).
- SATURATING arithmetic on all new paths.
- Line anchors are branch-base (`1b0efaf`); locate by quoted code, not line number, after earlier tasks shift the file.
- Every commit message ends with the trailers, verbatim (use `git commit -F -` with a quoted heredoc — `!` breaks zsh inside double-quoted `-m`):
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: the break engine — helper + word-wrap loop + laws (ONE commit)

**Files:**
- Modify: `wordcartel-core/Cargo.toml` (one dependency line), `wordcartel-core/src/layout.rs` (hoist `VG`, add helper, rework the loop, laws + tests); verify-only: `wordcartel/src/nav.rs` tests, `wordcartel/src/derive.rs` tests

**Interfaces:**
- Consumes: nothing new.
- Produces: the new wrap geometry every later task and consumer sees (`layout()`'s API unchanged); internally `fn visible_break_indices(vg_texts: &[&str]) -> Vec<usize>` + the hoisted file-scope `VG`.

**Why one commit (Codex plan r1):** the helper alone is dead code until the loop consumes
it — a helper-only commit fails the warning-free gate. Helper and loop land together;
the TDD stages below are sequenced inside the task.

- [ ] **Step 1: add the dependency.** In `wordcartel-core/Cargo.toml` under `[dependencies]`, beside the existing `unicode-segmentation`/`unicode-width` lines:

```toml
unicode-linebreak = "0.1"
```

Run `cargo build -p wordcartel-core` once so Cargo.lock updates (commit the lock change with this task).

- [ ] **Step 2: write the failing tests.** In layout.rs's `#[cfg(test)] mod tests`, add (names exact):

```rust
    #[test]
    fn break_indices_space_hyphen_emdash() {
        // "ab cd" → graphemes a,b,' ',c,d — opportunity BEFORE c (index 3, after the space run)
        assert_eq!(visible_break_indices(&["a", "b", " ", "c", "d"]), vec![3]);
        // hyphen inside a word: break after '-', i.e. before 'r' (index 5)
        assert_eq!(visible_break_indices(&["s", "e", "l", "f", "-", "r", "e", "f"]), vec![5]);
        // em-dash prose: " — " yields an opportunity after the trailing space (before 'b')
        assert_eq!(visible_break_indices(&["a", " ", "—", " ", "b"]), vec![2, 4]);
    }

    #[test]
    fn break_indices_nbsp_never_breaks() {
        assert_eq!(visible_break_indices(&["a", "\u{a0}", "b"]), Vec::<usize>::new());
    }

    #[test]
    fn break_indices_flag_pins_unicode_15_0_behavior() {
        // unicode-linebreak 0.1.5 = Unicode 15.0.0 (pre-LB20a): a word-initial hyphen
        // ALLOWS a break after it — "-flag" may wrap after '-'. Accepted wart (spec I1).
        assert_eq!(visible_break_indices(&["x", " ", "-", "f", "g"]), vec![2, 3]);
    }

    #[test]
    fn break_indices_drop_mid_cluster_offsets() {
        // Spec C1: " \u{301}" is ONE grapheme cluster to UAX #29 but UAX #14 puts a
        // break offset at the combining mark — mid-VG. The offset must be DROPPED.
        assert_eq!(visible_break_indices(&["a", " \u{301}", "b"]), Vec::<usize>::new());
    }

    #[test]
    fn break_indices_mandatory_midline_treated_as_allowed_and_eot_dropped() {
        // U+2028 survives inside a logical line (spec I2): its Mandatory break maps
        // like any Allowed one (offset lands on the VG after the separator)…
        assert_eq!(visible_break_indices(&["a", "\u{2028}", "b"]), vec![2]);
        // …and the end-of-text entry never appears.
        assert_eq!(visible_break_indices(&["a", "b"]), Vec::<usize>::new());
        assert_eq!(visible_break_indices(&[]), Vec::<usize>::new());
    }

    #[test]
    fn break_indices_cjk_between_ideographs() {
        // Mixed script: opportunities between ideographs and at the script seam.
        let v = visible_break_indices(&["中", "文", "E", "n"]);
        assert!(v.contains(&1), "between ideographs: {v:?}");
        assert!(v.contains(&2), "ideograph→latin seam: {v:?}");
    }
```

- [ ] **Step 3: run to verify RED.** `cargo test -p wordcartel-core break_indices` — FAIL: `visible_break_indices` not found.

- [ ] **Step 4: hoist `VG` and implement the helper.** Move the `struct VG { src, text, width, style }` declaration (currently inside `layout()`, layout.rs:211-216) to file scope unchanged (private; place it above `grapheme_width`, keeping its doc context — add one line `/// One visible grapheme after concealment (see layout()).`). Then add beside it:

```rust
/// UAX #14 break opportunities over the VISIBLE grapheme sequence, as indices
/// into the VG vector: index `i` means "a row may end before VG i". Offsets
/// that do not land on a VG start are DROPPED (UAX #14 and UAX #29 disagree at
/// e.g. space+combining-mark — one cluster to the segmenter, a break point to
/// the line breaker; dropping is conservative, never splitting a cluster). The
/// end-of-text entry is dropped likewise. Mid-line Mandatory entries (U+2028
/// et al. survive inside a logical line) are treated exactly like Allowed.
fn visible_break_indices(vg_texts: &[&str]) -> Vec<usize> {
    let mut concat = String::new();
    let mut starts: Vec<usize> = Vec::with_capacity(vg_texts.len());
    for t in vg_texts {
        starts.push(concat.len());
        concat.push_str(t);
    }
    let mut out: Vec<usize> = Vec::new();
    let mut cursor = 0usize; // starts[] is ascending; resume the scan per offset
    for (off, _op) in unicode_linebreak::linebreaks(&concat) {
        if off >= concat.len() {
            continue; // the end-of-text entry
        }
        while cursor < starts.len() && starts[cursor] < off {
            cursor += 1;
        }
        if cursor < starts.len() && starts[cursor] == off {
            out.push(cursor); // lands on a VG start — keep
        }
        // else: mid-VG offset — dropped (spec C1)
    }
    out
}
```

And the `use` line at the top of layout.rs's import block: none needed (the helper names the crate via full path `unicode_linebreak::linebreaks`).

- [ ] **Step 5: run to verify the helper tests GREEN.** `cargo test -p wordcartel-core break_indices` — all 6 pass. (The build may warn dead_code at THIS stage — expected mid-task; the loop lands before the commit.)

- [ ] **Step 6: write the failing wrap-loop tests** (layout.rs tests; names exact):

```rust
    #[test]
    fn word_wrap_breaks_at_space_not_midword() {
        // vw 8, no prefix: "hello wide" → "hello " / "wide" (space hangs? fits at col 5)
        let (rows, _) = layout("hello wide", BlockRole::Paragraph, false, 8, false);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].display, "hello ");
        assert_eq!(rows[1].display, "wide");
    }

    #[test]
    fn word_wrap_trailing_whitespace_hangs_past_edge() {
        // vw 4: "abcd " — the space lands at col 4 (== vw) and HANGS; one row.
        let (rows, map) = layout("abcd ", BlockRole::Paragraph, false, 4, false);
        assert_eq!(rows.len(), 1);
        assert_eq!(map.row_end_col[0], 5, "hang: end col past vw");
        // Law 4: the space is PLACED, never dropped.
        assert_eq!(map.placed.len(), 5);
    }

    #[test]
    fn word_wrap_fallback_when_no_opportunity() {
        // Unbroken token: byte-identical to the old greedy wrap.
        let (rows, _) = layout("abcdef", BlockRole::Paragraph, false, 4, false);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].display, "abcd");
        assert_eq!(rows[1].display, "ef");
    }

    #[test]
    fn word_wrap_codeblock_keeps_grapheme_wrap() {
        // Same text, CodeBlock role: spaces do NOT become break points.
        let (rows, _) = layout("let x = 1;", BlockRole::CodeBlock, false, 4, false);
        assert_eq!(rows[0].display, "let ", "greedy fill, mid-token break allowed");
        assert_eq!(rows[1].display, "x = ");
    }

    #[test]
    fn word_wrap_repeat_zero_width_head_no_overwide_row() {
        // Probe-confirmed spec-D2 repeat case: a zero-width head means the tail
        // re-place frees ZERO columns — the current VG must wrap again, never
        // producing an over-wide multi-grapheme row (Law 3).
        let (rows, _) = layout("\u{200b}ab", BlockRole::Paragraph, false, 1, false);
        assert!(rows.iter().all(|r| r.width <= 1 || r.display.chars().count() == 1),
            "no over-wide multi-grapheme row: {rows:?}");
    }

    #[test]
    fn word_wrap_codeblock_space_wraps_not_hangs() {
        // CodeBlock: the hang rule is OFF — a space at the edge wraps greedily,
        // byte-identical to today (spec D2 as amended).
        let (rows, _) = layout("abcd x", BlockRole::CodeBlock, false, 4, false);
        assert_eq!(rows[0].display, "abcd");
        assert_eq!(rows[1].display, " x");
    }

    #[test]
    fn word_wrap_break_at_row_start_falls_back() {
        // The only opportunity coincides with the row start (guard row_start_vg < break):
        // " abcdefgh" at vw 4 — opportunity at VG 1 only; rows after the first break
        // have no interior opportunity → grapheme fallback, no infinite loop.
        let (rows, _) = layout(" abcdefgh", BlockRole::Paragraph, false, 4, false);
        assert!(rows.len() >= 3, "must terminate and cover: {rows:?}");
    }
```

- [ ] **Step 7: run to verify RED.** `cargo test -p wordcartel-core word_wrap` — the space-break, break-at-row-start (and possibly others) fail against the greedy loop (`hello wi`/`de`; CodeBlock is identical today so that one may pass — confirm which fail and record it).

- [ ] **Step 8: rework the loop.** Replace layout.rs:260-294 — through and INCLUDING the existing `row_end_col.push(col); let rows = row + 1;` tail, which the snippet re-supplies (Fable plan M-1: stopping at :292 duplicates those two lines) — with:

```rust
    // Word-boundary soft-wrap (UAX #14; spec D1/D2). CodeBlock keeps grapheme wrap.
    let breaks: Vec<usize> = if matches!(role, BlockRole::CodeBlock) {
        Vec::new()
    } else {
        let texts: Vec<&str> = vgs.iter().map(|v| v.text.as_str()).collect();
        visible_break_indices(&texts)
    };
    let mut placed: Vec<Placed> = Vec::new();
    let mut row = 0usize;
    let mut col = prefix_width;
    let mut row_end_col: Vec<usize> = Vec::new();
    let mut row_start_vg = 0usize; // first VG index on the current row

    for (i, vg) in vgs.iter().enumerate() {
        if vg.width == 0 {
            placed.push(Placed { src: vg.src.clone(), row, col, width: 0, text: vg.text.clone(), style: vg.style });
            continue;
        }
        let is_ws = vg.text == " " || vg.text == "\t";
        // The hang rule is scoped OFF for CodeBlock (spec D2 as amended: in code a
        // space/tab is data — byte-identical wrap preserved).
        let hang = is_ws && !matches!(role, BlockRole::CodeBlock);
        // The overflow decision REPEATS until the current VG fits (spec D2 as amended,
        // user-ratified from a probe-confirmed Fable Critical): a tail re-placement can
        // leave the current VG still over-wide (zero-width head; no-break-before tail).
        // Each pass either advances the break point strictly or falls back at the row
        // start, where the single-grapheme guard ends the loop — termination guaranteed.
        while !hang && col + vg.width > vw && col > prefix_width {
            // Largest legal break k with row_start_vg < k <= i (breaks is ascending):
            // stateless O(log n) lookup — a per-row cursor that resets on re-placement
            // silently DROPS breaks between the chosen one and i (a W1 violation).
            let cut = breaks.partition_point(|&k| k <= i);
            let cand = breaks[..cut].last().copied().filter(|&k| k > row_start_vg);
            match cand {
                // The break is exactly the CURRENT (unpushed) VG: the row ends here
                // with NO tail to re-place — placed[i] does not exist yet (Codex plan
                // r1 Critical: indexing it panics on e.g. "- aaaa bbbb" @ 6, where the
                // break before 'bbbb' meets the overflow at 'b').
                Some(b) if b == i => {
                    row_end_col.push(col);
                    row += 1;
                    col = prefix_width;
                    row_start_vg = i;
                }
                // A legal break strictly inside this row: end the row there and
                // re-place the tail (break..i) onto the new row (spec D2). `placed`
                // has exactly one entry per VG (zero-widths included), so the break
                // VG's placed index IS its VG index.
                Some(b) => {
                    row_end_col.push(placed[b].col);
                    row += 1;
                    let mut c = prefix_width;
                    for p in placed[b..].iter_mut() {
                        p.row = row;
                        p.col = c;
                        c += p.width;
                    }
                    col = c;
                    row_start_vg = b;
                }
                // No interior opportunity: grapheme fallback (today's behavior).
                None => {
                    row_end_col.push(col);
                    row += 1;
                    col = prefix_width;
                    row_start_vg = i;
                }
            }
        }
        placed.push(Placed { src: vg.src.clone(), row, col, width: vg.width, text: vg.text.clone(), style: vg.style });
        col += vg.width;
    }
    row_end_col.push(col);
    let rows = row + 1;
```

Notes the implementer must honor: whitespace VGs (`is_ws`) bypass the overflow test entirely — they hang (spec D2); zero-width VGs at/after the break travel with the tail (they sit in `placed[b..]` by index — `placed` is index-parallel to `vgs`); `row_end_col` for the broken row is `placed[b].col` in the tail-re-place arm, and the current `col` in both the b==i and fallback arms (in every case: the col after the last VG that stays — hanging whitespace included). Everything after the loop (:293-:346, the `visual_rows` build) is UNCHANGED.

- [ ] **Step 9: run to verify GREEN** — `cargo test -p wordcartel-core word_wrap` (and the break_indices six stay green; the dead_code warning gone now the loop consumes the helper), then the wrap-sensitive existing tests: `active_line_identity_and_wrap` must pass UNCHANGED (no opportunity in `abcdef`; add the one-line comment `// no UAX #14 opportunity — pins the grapheme fallback` above it); `prefix_reduces_wrap_capacity` UPDATE per spec: `"- aaaa bbbb"` @ 6 → row 0 displays `aaaa ` (space hanging, end col 7), row 1 `bbbb` at col 2 — rewrite its assertions to exactly that.

- [ ] **Step 10: amend Law 3, add W1, extend the strategy.** In `law3_softwrap_fidelity` (layout.rs:838-882), replace the width assertion with the composable form (spec D4/I3). **The real law body's locals are `sum`, `row`, `ri`, `on_row` (:860-868) and proptest bodies use `prop_assert!` — adapt this LOGIC to those exact names and macros; the snippet below is the logic, not a drop-in (Codex plan r1):**

```rust
            // Composable width bound (spec Law 3): row width MINUS trailing-whitespace
            // width must fit, unless the non-whitespace content is one over-wide grapheme.
            let trailing_ws: usize = on_row.iter().rev()
                .take_while(|p| p.text == " " || p.text == "\t")
                .map(|p| p.width).sum();
            let non_ws_count = on_row.iter().filter(|p| !(p.text == " " || p.text == "\t")).count();
            let content_width = sum - trailing_ws; // `sum` = the row's total width in the real body
            prop_assert!(
                content_width <= w || non_ws_count == 1,
                "row {}: content {} > vw {} with {} non-ws graphemes", ri, content_width, w, non_ws_count
            );
```

Add law W1 as a new proptest beside it (over the same strategy), asserting for every non-CodeBlock row boundary with first VG index `j`: `breaks.contains(&j) || !breaks.iter().any(|&k| row_start < k && k <= j)` — recompute `breaks` in the test via `visible_break_indices` on the laid-out line's VG texts (expose the VG texts by re-deriving them from `map.placed` in row order: their `text` fields ARE the VG texts). Extend `token()` (layout.rs:715-736) with the bare combining-mark token `Just("\u{301}".to_string())` (spec C1).

- [ ] **Step 11: verify the neighbors.** Run the full core suite plus: each of `cargo test -p wordcartel screen_pos_wrapped_line_second_visual_row`, `… caret_in_tall_wrapped_line_stays_visible`, `… long_line_wraps_at_small_width`, `… rebuild_fills_editing_rows` (separate runs — cargo test takes one positional filter; Fable plan M-3) — ALL must pass UNCHANGED (their corpora have no break opportunities; if one fails, STOP — that is a real geometry bug, not a test to update). Then the full gates.

- [ ] **Step 12: commit** — `feat(b1): word-boundary wrap — UAX #14 break engine, whitespace hang, grapheme fallback, CodeBlock exemption; Law 3 composable + W1`.

---

### Task 2: B2 — tab-aware, marker-conditional list-indent conceal

**Files:**
- Modify: `wordcartel-core/src/md_parse.rs` (the ListItem arm :252-:292 + tests), `wordcartel-core/src/layout.rs` (W2 test only)

**Interfaces:**
- Consumes: nothing from Task 1 (independent change; sequenced after so wrap geometry is settled when its tests pin hanging indent).
- Produces: nested-list glyphs `"<indent>• "` / `"<indent><ordinal>. "`; `prefix_width` now includes indent for nested items.

- [ ] **Step 1: write the failing tests** (md_parse.rs tests; names exact):

```rust
    #[test]
    fn nested_unordered_indent_concealed_into_glyph() {
        let a = analyze("  - sub", BlockRole::ListItem, false);
        assert_eq!(visible_text(&a, "  - sub"), "sub");
        assert_eq!(a.prefix_glyph.as_deref(), Some("  • "));
    }

    #[test]
    fn tab_indented_item_recognized_and_expanded() {
        // A leading tab is indent (spec D3: the scan is now tab-aware) and expands
        // to TAB_WIDTH spaces in the glyph so widths match the old visual layout.
        let a = analyze("\t- sub", BlockRole::ListItem, false);
        assert_eq!(visible_text(&a, "\t- sub"), "sub");
        assert_eq!(a.prefix_glyph.as_deref(), Some("    • "));
    }

    #[test]
    fn nested_ordered_indent_concealed_into_glyph() {
        let a = analyze("   2. x", BlockRole::ListItem, false);
        assert_eq!(visible_text(&a, "   2. x"), "x");
        assert_eq!(a.prefix_glyph.as_deref(), Some("   2. "));
    }

    #[test]
    fn markerless_listitem_continuation_keeps_indent_no_glyph() {
        // Continuation lines of a multi-line item carry ListItem role with no marker
        // (spec I4): indent must stay VISIBLE and no glyph appear — else invisible text.
        let a = analyze("  second", BlockRole::ListItem, false);
        assert_eq!(visible_text(&a, "  second"), "  second");
        assert_eq!(a.prefix_glyph, None);
    }
```

(The module's real helper is named `visible` (md_parse.rs:369-374, Codex plan r1) — use it; the test code above writes `visible_text` for readability, substitute the real name.)

- [ ] **Step 2: run to verify RED.** `cargo test -p wordcartel-core nested_ tab_indented markerless_` — the first three fail (glyph `"• "`, indent visible); the fourth passes today — keep it as the guard pin and note that in the report.

- [ ] **Step 3: implement the arm.** Replace the `BlockRole::ListItem` arm (md_parse.rs:252-292, quoted verbatim in the current code) with:

```rust
        BlockRole::ListItem => {
            // Skip leading indent: spaces AND tabs (spec D3 — the scan is tab-aware).
            let start = bytes.iter().take_while(|&&b| b == b' ' || b == b'\t').count();
            if start >= n {
                return None;
            }
            // The glyph reproduces the indent's display width: space as-is, tab as
            // TAB_WIDTH spaces (matches layout's tab policy) — so the bullet paints
            // at its indent level and continuation rows hang under the item text.
            let indent_str: String = bytes[..start]
                .iter()
                .map(|&b| if b == b'\t' { "    " } else { " " })
                .collect();
            let b0 = bytes[start];

            // Unordered marker: `[-*+]` followed by space or tab
            if (b0 == b'-' || b0 == b'*' || b0 == b'+')
                && start + 1 < n
                && is_ws(bytes[start + 1])
            {
                // Conceal indent + marker + its whitespace (marker-conditional: the
                // no-marker path below conceals NOTHING — spec I4).
                visible[..start].fill(false);
                visible[start] = false;
                visible[start + 1] = false;
                return Some(format!("{indent_str}\u{2022} "));
            }

            // Ordered marker: `<digits>[.)]` followed by space or tab
            if b0.is_ascii_digit() {
                let digit_end = bytes[start..]
                    .iter()
                    .take_while(|&&b| b.is_ascii_digit())
                    .count()
                    + start;
                if digit_end < n
                    && (bytes[digit_end] == b'.' || bytes[digit_end] == b')')
                    && digit_end + 1 < n
                    && is_ws(bytes[digit_end + 1])
                {
                    let ordinal: &str = &line[start..digit_end];
                    let glyph = format!("{indent_str}{ordinal}. ");
                    visible[..start].fill(false);
                    visible[start..=digit_end + 1].fill(false);
                    return Some(glyph);
                }
            }

            None
        }
```

(`\u{2022}` is `•` — write the literal `•` in the source, matching the current arm's style; the escape here is for plan-transport only. TAB_WIDTH is 4 — if md_parse.rs cannot see layout's `TAB_WIDTH` const, write the literal four spaces with a comment `// TAB_WIDTH = 4 (layout.rs tab policy)`; do NOT add a cross-module dependency for one constant.)

- [ ] **Step 4: run to verify GREEN** — the four new tests plus the existing list tests (:461, :491, :522, :530) all pass unchanged (they use unindented items).

- [ ] **Step 5: add W2** (layout.rs tests — a parametrized unit test, spec D4/M5):

```rust
    #[test]
    fn law_w2_nested_prefix_alignment_inactive() {
        for (line, marker_w) in [("- x", 2), ("  - x", 2), ("    - x", 2), ("\t- x", 2),
                                 ("1. x", 3), ("   12. x", 4)] {
            let indent_w = line.bytes().take_while(|&b| b == b' ' || b == b'\t')
                .map(|b| if b == b'\t' { 4 } else { 1 }).sum::<usize>();
            let (rows, map) = layout(line, BlockRole::ListItem, false, 20, false);
            assert_eq!(map.prefix_width, indent_w + marker_w, "{line:?}");
            assert!(map.placed.iter().all(|p| p.col >= map.prefix_width), "{line:?}");
            assert_eq!(rows[0].prefix_glyph.as_deref().map(|g| !g.is_empty()), Some(true));
        }
    }
```

- [ ] **Step 6: full gates.**

- [ ] **Step 7: commit** — `feat(b2): nested-list indent folds into the prefix glyph — tab-aware, marker-conditional; W2 alignment law`.

---

### Task 3: render caret clamp, composition pins, e2e journeys

**Files:**
- Modify: `wordcartel/src/render.rs` (the cursor-set site :714-:718 + tests), `wordcartel/src/nav.rs` (one stale doc comment :1484-:1485), `wordcartel/src/e2e.rs` (journeys)

**Interfaces:**
- Consumes: Task 1's geometry, Task 2's glyphs.
- Produces: the shipped behavior; no API changes.

- [ ] **Step 1: write the failing render test** (render.rs tests, TestBackend idiom of the neighbors):

First a test-local helper modeled EXACTLY on the module's existing `render_to_buffer`
(same construction, same draw call — verify the draw fn's real name there), plus a
cursor read:

```rust
    fn render_capturing_cursor(e: &mut Editor, w: u16, h: u16) -> Option<(u16, u16)> {
        // Same shape as render_to_buffer, but reads the backend cursor after draw.
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut term = ratatui::Terminal::new(backend).unwrap();
        term.draw(|f| /* the same draw call render_to_buffer makes */).unwrap();
        // TestBackend's INHERENT cursor_position() — no Backend trait import needed
        // (Codex plan r1; the trait method get_cursor_position would need `use
        // ratatui::backend::Backend`, which the test module does not import).
        let p = term.backend().cursor_position();
        Some((p.x, p.y))
    }
```

(If the inherent method's exact name differs in the vendored ratatui, check
TestBackend's inherent API — it exists per ratatui-core test.rs:112-118 — and do NOT
import the Backend trait.)

Then the pin:

```rust
    #[test]
    fn hung_trailing_space_caret_pins_at_edge() {
        // vw 4: "abcd " hangs its trailing space (placed at col 4 == vw); the caret
        // AFTER the space (byte 5, eol) maps to logical col 5 — past the text rect.
        // Today's `col < text_width` guard suppresses the cursor entirely (invisible
        // caret while typing "word "); the clamp pins it at text_width-1 (spec D2).
        let mut e = Editor::new_from_text("abcd \n", None, (4, 8));
        set_caret(&mut e, 5); // eol, after the hung space (active line — layout raw)
        derive::rebuild(&mut e);
        let (col, _row) = crate::nav::screen_pos(&e).expect("caret on screen");
        assert!(col as usize >= 4, "precondition: logical col past the rect, got {col}");
        let cur = render_capturing_cursor(&mut e, 4, 8);
        let (x, _y) = cur.expect("helper returns Some; suppression shows as (0,0)");
        assert_eq!(x, 3, "pinned at text_width-1 (text_left is 0 here)");
    }
```

RED expectation: TestBackend initializes the cursor at (0,0) and today's guard never
sets it — so `x == 0`, and the `assert_eq!(x, 3)` fails (Codex plan r1: the position
always reads back; it is the VALUE that discriminates, not Some/None).

- [ ] **Step 2: implement the clamp.** At render.rs:714-718, replace:

```rust
    } else if let Some((col, row)) = nav::screen_pos(editor) {
        // Guard: only set if within the editing area (not into the status line).
        if row < edit_height && col < tg.text_width {
            frame.set_cursor_position(Position { x: area.x + tg.text_left + col, y: edit_top + row });
        }
    }
```

with:

```rust
    } else if let Some((col, row)) = nav::screen_pos(editor) {
        // Guard rows; clamp cols — a caret on/after hung trailing whitespace sits
        // logically past the text rect and pins at the edge (spec D2 clamp).
        if row < edit_height && tg.text_width > 0 {
            let col = col.min((tg.text_width as usize).saturating_sub(1) as u16);
            frame.set_cursor_position(Position { x: area.x + tg.text_left + col, y: edit_top + row });
        }
    }
```

(Adjust integer types to the site's real ones — `col`/`text_width` are u16 there; keep the zero-width suppression the old guard provided via `tg.text_width > 0`.)

- [ ] **Step 3: GREEN + neighbors.** The new test passes; `wrapped_list_item_continuation_row_aligns_text_and_caret` passes UNCHANGED (spec M6: its geometry is reproduced byte-identically under word wrap — if it fails, STOP and investigate, do not update it).

- [ ] **Step 4: the composition pin** (render.rs tests) — the effort's headline case:

```rust
    #[test]
    fn wrapped_nested_item_bullet_column_and_hanging_indent() {
        // The effort's headline composition (B1 × B2). 12-wide, "  - alpha beta":
        // glyph "  • " (indent 2 + bullet 2 = prefix_width 4);
        //   row 0: "  • alpha "  (bullet at col 2, text from col 4, space hangs ok)
        //   row 1: "    beta"    (spacer cols 0..4, text at col 4 — under TEXT)
        let mut e = Editor::new_from_text("  - alpha beta\nmore\n", None, (12, 8));
        set_caret(&mut e, 17); // on "more" so line 0 is INACTIVE (conceal active)
        derive::rebuild(&mut e);
        {
            let (_rows, map) = &e.active().view.line_layouts[&0];
            assert_eq!(map.prefix_width, 4, "indent(2) + bullet(2)");
            assert!(map.rows >= 2, "must wrap");
        }
        let buf = render_to_buffer(&mut e, 12, 8);
        assert_eq!(buf[(2u16, 0u16)].symbol(), "\u{2022}", "bullet at indent col 2");
        assert_eq!(buf[(4u16, 0u16)].symbol(), "a", "item text at col 4");
        for c in 0..4u16 {
            assert_eq!(buf[(c, 1u16)].symbol(), " ", "continuation spacer col {c}");
        }
        assert_eq!(buf[(4u16, 1u16)].symbol(), "b", "continuation hangs under TEXT");
        // Round-trip on the continuation row: "beta" starts at byte 10.
        let (_rows, map) = &e.active().view.line_layouts[&0];
        let (vrow, vcol) = map.source_to_visual(10);
        assert_eq!((vrow, vcol), (1, 4));
        assert_eq!(map.visual_to_source(1, 4), 10);
    }
```

(Write the bullet literal `•` in source, not the escape; byte arithmetic: `"  - alpha beta"`
= indent 2 + marker 2 + `alpha`(4..9) + space(9) + `beta`(10..14); caret 17 lands in
`more`. Hand-walked: alpha at cols 4-8, space at 9, `be` reach col 12, `t` overflows,
break at VG 6 (`b`) → row 1 = `beta` at cols 4-7.)

- [ ] **Step 5: e2e journeys** (e2e.rs, Harness idiom):
  - `journey_typing_never_breaks_midword`: open a narrow-width doc, type `the quick brown fox jumps over` past the edge → assert via `screen_contains` that no rendered row ends mid-word (check the specific expected rows), caret visible; End/Home/up/down navigate across the wrap without panic and land where `screen_pos` says.
  - `journey_nested_list_wraps_hanging`: type `  - ` then enough words to wrap, THEN
    move the caret off the item line (Enter or Down — the ACTIVE line renders raw with
    no glyph, Fable plan I-2) → bullet at indent col, continuation under text (assert
    the two specific expected screen rows).

- [ ] **Step 6: the stale comment.** nav.rs:1484-1485 (`typewriter_rows_prefix_aware`'s doc comment): update its column arithmetic to the new geometry (the break follows the tab; assertions themselves survive — spec M6). Verify that test passes unmodified apart from the comment.

- [ ] **Step 7: full gates** + `scripts/smoke/run.sh` (quote the one-line summary in the report — advisory).

- [ ] **Step 8: commit** — `feat(b1b2): caret edge clamp, wrapped-nested-item composition pins, e2e wrap journeys`.

---

## Verification appendix (final whole-branch review charge)

- Laws: 3 (composable form), 4, 5, W1 (test name MUST contain `law_w1`), W2 all green
  under the extended strategy (bare `\u{301}` token present). For the raised-count run:
  the suite pins `cases: 512` in `proptest_config` (layout.rs:752-755), which OVERRIDES
  the env var (Fable plan M-3) — temporarily edit the config to 2048, run
  `cargo test -p wordcartel-core law_`, quote the result, and REVERT the edit before
  committing. Run multi-test filters as separate invocations (cargo test takes one
  positional filter).
- Spec-coverage delegations (Fable plan M-4): the long-URL fallback and a layout-level
  CJK mixed-script wrap pin ride in Task 1's unit tests (add
  `word_wrap_long_url_falls_back` and `word_wrap_cjk_mixed_script` as two more cases in
  Step 6, same shape as the existing five — the implementer derives expectations from
  the helper's probe-verified vectors); ship-time bookkeeping (backlog B1+B2 SHIPPED
  entries incl. the CodeBlock exception and the unicode-linebreak dependency/wart notes,
  memory working-order advance) is the CONTROLLER's merge-time step, not a task.
- The spec's "unchanged" pins really unchanged: `active_line_identity_and_wrap`, both nav wrap tests, `wrapped_list_item_continuation_row_aligns_text_and_caret`, derive's two, the four md_parse unindented list tests, `markerless_listitem_continuation_keeps_indent_no_glyph`.
- Hot path: no new work on lines that don't overflow beyond the linebreaks pass + concat (O(visible)); LayoutKey cache untouched.
- Grep: no `#[allow]` added; no `unsafe`; `unicode-linebreak` appears only in wordcartel-core.
- Pre-merge: smoke verbatim; a live tmux sanity — narrow terminal, type a sentence past the edge, confirm word wrap on screen and `  - ` nesting renders `  • ` with hanging continuation.
