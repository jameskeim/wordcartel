# B17 — Soft-wrap trailing-space-at-margin caret feedback (implementation plan)

**For agentic workers:** execute the tasks IN ORDER. Each task is TDD (write/adjust the failing test →
implement → green → commit) and is an independently-testable, independently-committable deliverable.
Keep every commit green. Anchor on symbol NAMES, not the `:NNN` line numbers below — they drift as
edits land; re-locate with `grep`/`documentSymbol`. For any compile/usage question on code you are
editing, trust `cargo`, not an editor "unused/undefined" hint. Design note (Codex-clean):
`docs/superpowers/specs/2026-07-16-b17-softwrap-trailing-space-break-design.md`.

## Goal

Make a trailing space typed at the soft-wrap margin give real caret feedback: the space still hangs on
its row, but `layout()` appends one **empty flush continuation row** whose col 0 (or `prefix_width`) the
post-space caret occupies. The next typed word lands at col 0; today the caret pins to the word-end and
the space reads as "didn't register" (→ stray double-spaces). This **amends spec D2**.

## Architecture

The soft-wrap engine `wordcartel_core::layout::layout` (`layout.rs:244`) already places a hung trailing
space past the margin; the only change is a ~6-line post-loop epilogue that appends an empty visual row
when the final row ends in a whitespace run at/past the margin (non-CodeBlock). The caret then falls out
of the EXISTING `ColMap` resolvers unchanged — `source_to_visual`'s `offset >= eol` branch
(`layout.rs:87-90`) returns the phantom row at `prefix_width`, and `visual_to_source`'s empty-row branch
(`layout.rs:148-155`) handles click/overshoot. Shell consumers (`render.rs`, `nav.rs`, `ventilate.rs`)
need no logic change — only rewritten expectations and one clamp comment.

## Tech Stack

Rust (workspace: `wordcartel-core` pure lib + `wordcartel` shell). Tests: `#[cfg(test)]` unit modules +
proptest (`layout.rs mod props`) + ratatui `TestBackend` render goldens. No new dependencies.

## Global Constraints

- **House style, hand-formatted.** Match neighbors; 4-space indent; hand-wrapped ~100-col; em-dash `—`
  in prose comments, never `--`. **Do NOT run `cargo fmt`** (repo has no `rustfmt.toml`; it would reflow
  the tree). Do not reflow code you did not otherwise change.
- **`wordcartel-core` is `#![forbid(unsafe_code)]`** and pure — no I/O, no shell types.
- **Merge GATEs:** `cargo test` green across all suites; `cargo build` + `cargo test --no-run`
  warning-free for touched crates; **workspace clippy clean** (`cargo clippy --workspace --all-targets`,
  `[workspace.lints.clippy] all = "deny"` — prefer `is_some_and` over `map_or(false, …)`);
  `module_budgets` test; backlog-drift test. The core change lives inside the existing
  `#[allow(clippy::too_many_lines)]` on `layout()` — add no new hub bulk.
- **The six wrap property-laws MUST stay green UNMODIFIED.** The design note claims this by walking each
  assertion (space stays placed → Law 4; phantom row width 0 ≤ w → Law 3(c); the `.min(max_row+1)` at
  `layout.rs:1143` tolerates a trailing empty row; empty-row nav branches cover Laws 2/5/6). **Task 1
  CONFIRMS it by running `cargo test -p wordcartel-core props` (or the whole layout test module) and
  stating all six pass** — a run, not an assertion. Do NOT edit any `law*` test.
- **command-surface-contract: N/A** — B17 touches no commands, options, palette, menu, or hints; it
  changes only pure layout geometry.
- **The rule (all five forks resolved "A"):** *phantom flush row* — the break-space stays hung at the
  end of row N; append one empty flush visual row N+1 at `prefix_width`. Trigger: **the final row ends in
  a whitespace run whose end column ≥ vw, and `role != CodeBlock`.** Multi-space run → one phantom row,
  caret parks at (N+1, col 0) for the whole run. Uniform across all render modes and at EOF (the rare
  visible blank row in a concealed hard-break line reaching the margin is accepted). Exact-fill (end col
  `== vw`) is included.
- **Commit trailers** — every commit ends with, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  Claude-Session: <current session URL>
  ```
- Branch `effort-b17-softwrap-trailing-space` (design note already committed at e70873c). Commit per
  task; push only when asked.

---

## Task 1 — Layout epilogue + core-layout tests + render-golden rewrite (`wordcartel-core/src/layout.rs` + `wordcartel/src/render.rs`)

**Deliverable:** the phantom-flush-row append in `layout::layout`, the rewritten hang test, nine new
core-layout tests, updated in-code "spec D2" comments, the rewritten render golden + clamp comment, and
a CONFIRMED run of all six property laws.

**Why the render golden is in Task 1 (workspace-green as committed):** the epilogue changes `layout()`
in `wordcartel-core`, which makes the existing render golden `hung_trailing_space_caret_pins_at_edge`
(`render.rs:2005`, asserts caret `x == 3`) FAIL. It is the ONLY red test (no e2e/nav wrap journey shares
that trailing-space-at-margin assertion). Rewriting it in the SAME commit keeps `cargo test --workspace`
green at Task 1's commit.

**Interfaces (real signatures — do not change):**
- `pub fn layout(line: &str, role: BlockRole, render: LineRender, viewport_width: usize, heading_prefix: bool, reserve_cols: usize) -> (Vec<VisualRow>, ColMap)` — `layout.rs:244`. Locals in scope at the epilogue: `row: usize` (mut), `col: usize`, `placed: Vec<Placed>`, `row_end_col: Vec<usize>`, `prefix_width: usize`, `vw: usize` (`= viewport_width.max(1)`, `:252`).
- `pub struct Placed { pub src: Range<usize>, pub row: usize, pub col: usize, pub width: usize, pub text: String, pub style: Style }` — `layout.rs:13`.
- `pub struct ColMap { pub placed, pub rows: usize, pub eol: usize, pub row_end_col: Vec<usize>, pub is_active: bool, pub prefix_width: usize }` — `layout.rs:65`; `source_to_visual` `:86`, `visual_to_source` `:114`, `is_cursor_stop` `:159`.
- `pub struct Cursor { pub offset, pub row, pub desired_col }` `:445`; `cursor_at` `:469`, `move_left` `:491`, `move_home` `:504`, `move_end` `:518`, `move_down_within` `:529`, `move_up_within` `:541`.
- `BlockRole::{Paragraph, ListItem, CodeBlock}`, `LineRender::Concealed`, `TAB_WIDTH = 4` (`layout.rs:9`).
- `pub fn screen_pos(editor: &Editor) -> Option<(u16, u16)>` — `nav.rs:83`. `render.rs mod tests` helpers: `set_caret(&mut Editor, usize)` `:839`, `render_capturing_cursor(&mut Editor, u16, u16) -> Option<(u16,u16)>` `:858`. The render clamp arm: `render.rs:452-457`.

### 1a. The epilogue

Insert BETWEEN `row_end_col.push(col);` (`layout.rs:377`) and `let rows = row + 1;` (`:378`):

```rust
    row_end_col.push(col);
    // B17 (amends spec D2): a trailing whitespace run hung AT/PAST the margin gets an empty flush
    // continuation row, so the caret after the space lands at (next_row, prefix_width) = col 0 of the
    // text column — real line-move feedback that the space registered, with no leading-space indent.
    // Non-CodeBlock only: code preserves byte-identical wrap (the hang rule is already off for it, and
    // a code-block trailing space wraps to the next row via grapheme fallback — never hangs).
    if !matches!(role, BlockRole::CodeBlock)
        && col >= vw
        && placed.last().is_some_and(|p| p.text == " " || p.text == "\t")
    {
        row += 1;
        row_end_col.push(prefix_width);
    }
    let rows = row + 1;
```

Why this is complete and correct: `col` IS the just-pushed `row_end_col[final]` (end column of the final
row); `vw` is the same clamped bound the hang `while` uses (`:332`), so "at/past the margin" is defined
identically; `placed.last()` being a space/tab distinguishes a hung trailing space from a hung over-wide
grapheme (out of scope); `prefix_width` is the hanging-indent column (0 paragraph, 2 list, 6 ventilate).
The empty `VisualRow` is materialized for free by `vec![VisualRow { display: String::new(), .. }; rows]`
(`:380-381`) now that `rows` includes the phantom. No `ColMap` resolver edits.

### 1b. Rewrite the hang test

Replace `word_wrap_trailing_whitespace_hangs_past_edge` (`layout.rs:658-665`) with:

```rust
    #[test]
    fn word_wrap_trailing_whitespace_hangs_then_breaks_to_flush_row() {
        // B17 (amends spec D2): vw 4, "abcd " — the space still HANGS at col 4 (== vw) on row 0
        // (Law 4: placed, not dropped), but layout appends an empty flush continuation row, so the
        // caret after the space lands at (row 1, col 0) — feedback, with no leading-space indent.
        let (rows, map) = layout("abcd ", BlockRole::Paragraph, LineRender::Concealed, 4, false, 0);
        assert_eq!(rows.len(), 2, "hang-then-break appends a flush continuation row");
        assert_eq!(map.rows, 2);
        assert_eq!(map.row_end_col[0], 5, "space still hung past the edge on row 0");
        assert_eq!(map.row_end_col[1], 0, "phantom row flush at prefix_width (0 here)");
        assert_eq!(map.placed.len(), 5, "Law 4: the space is placed, never dropped");
        assert_eq!(map.source_to_visual(5), (1, 0), "caret after the space → col 0 of the flush row");
        assert_eq!(rows[1].display, "", "the flush row is empty");
    }
```

### 1c. New core-layout tests (append to `mod tests`)

```rust
    #[test]
    fn word_wrap_trailing_space_run_hangs_once_and_breaks() {
        // Fork b: "abcd   " @ vw 4 — the WHOLE run hangs on row 0; ONE flush row is appended and the
        // caret parks at (1, 0) for the entire run (extra spaces do not advance it).
        let (rows, map) = layout("abcd   ", BlockRole::Paragraph, LineRender::Concealed, 4, false, 0);
        assert_eq!(rows.len(), 2);
        assert_eq!(map.placed.len(), 7, "all three spaces placed on row 0 (Law 4)");
        assert_eq!(map.row_end_col[0], 7, "run hung past the edge");
        assert_eq!(map.source_to_visual(7), (1, 0), "caret parks at col 0 of the single flush row");
    }

    #[test]
    fn word_wrap_trailing_space_exact_fill_breaks() {
        // Fork d: "abc " @ vw 4 — the space lands exactly AT the last cell (end col == vw == 4). The
        // rule is "end col >= vw", so exact-fill triggers the flush row too.
        let (rows, map) = layout("abc ", BlockRole::Paragraph, LineRender::Concealed, 4, false, 0);
        assert_eq!(rows.len(), 2);
        assert_eq!(map.row_end_col[0], 4, "space at the last cell, end col == vw");
        assert_eq!(map.source_to_visual(4), (1, 0)); // eol = 4
    }

    #[test]
    fn word_wrap_trailing_tab_breaks_to_flush_row() {
        // A hung trailing TAB (width TAB_WIDTH == 4) past the margin also triggers the flush row.
        let (rows, map) = layout("abc\t", BlockRole::Paragraph, LineRender::Concealed, 4, false, 0);
        assert_eq!(rows.len(), 2);
        assert_eq!(map.row_end_col[0], 7, "tab hung past the edge (col 3 + width 4)");
        assert_eq!(map.source_to_visual(4), (1, 0)); // eol = 4 ("abc\t")
    }

    #[test]
    fn word_wrap_trailing_space_flush_row_hangs_to_prefix_width() {
        // List item "- aaaa " @ vw 6, prefix "• " (width 2): the trailing space hangs, and the flush
        // continuation row sits at prefix_width (col 2) — flush under the TEXT column, not screen col 0.
        let (rows, map) = layout("- aaaa ", BlockRole::ListItem, LineRender::Concealed, 6, false, 0);
        assert_eq!(rows.len(), 2);
        assert_eq!(map.prefix_width, 2, "• + space");
        assert_eq!(map.row_end_col[1], 2, "flush row at the hanging indent");
        assert_eq!(map.source_to_visual(7), (1, 2)); // eol = 7 ("- aaaa ")
    }

    #[test]
    fn word_wrap_codeblock_trailing_space_unchanged_no_flush_row() {
        // The epilogue does NOT fire for CodeBlock (guard !matches!(role, CodeBlock)). CodeBlock
        // disables the hang (:325) and has no break opportunities (:304-307), so a trailing space
        // ALREADY wraps to row 1 today via grapheme fallback — B17 adds no extra row (byte-identical).
        let (rows, map) = layout("abcd ", BlockRole::CodeBlock, LineRender::Concealed, 4, false, 0);
        assert_eq!(rows.len(), 2, "space already wrapped onto row 1 (unchanged from pre-B17)");
        assert_eq!(map.row_end_col, vec![4, 1], "row 0 ends at vw; the space sits at (1, 0), end col 1");
        let space = map.placed.last().unwrap();
        assert_eq!((space.row, space.col), (1, 0), "space wrapped, not hung; no phantom row appended");
    }

    #[test]
    fn word_wrap_whitespace_only_line_at_margin_breaks() {
        // Fork c (uniform): a whitespace-only line that reaches the margin gains a flush row too.
        let (rows, map) = layout("    ", BlockRole::Paragraph, LineRender::Concealed, 4, false, 0);
        assert_eq!(rows.len(), 2);
        assert_eq!(map.row_end_col[0], 4, "four spaces fill the row to the margin");
        assert_eq!(map.source_to_visual(4), (1, 0)); // eol = 4
    }

    #[test]
    fn word_wrap_trailing_space_within_margin_no_flush_row() {
        // Negative: a trailing space well within the margin does NOT trigger the flush row (end col < vw).
        let (rows, map) = layout("ab ", BlockRole::Paragraph, LineRender::Concealed, 8, false, 0);
        assert_eq!(rows.len(), 1, "space at col 2 < vw 8 — no phantom row");
        assert_eq!(map.row_end_col[0], 3);
    }

    #[test]
    fn word_wrap_trailing_space_hang_interior_caret_still_pins() {
        // The caret BEFORE the hung space (byte 4 of "abcd ") stays on the hung cell (0, 4) — render
        // clamps it to the edge. move_left from eol lands there (the pinned hang edge).
        let (_rows, map) = layout("abcd ", BlockRole::Paragraph, LineRender::Concealed, 4, false, 0);
        assert_eq!(map.source_to_visual(4), (0, 4), "before-space caret on the hung cell");
        let left = move_left(&map, cursor_at(&map, map.eol));
        assert_eq!(left.offset, 4, "left from eol lands on the hung space's start");
        assert_eq!(map.source_to_visual(left.offset), (0, 4));
    }

    #[test]
    fn phantom_flush_row_nav_lands_on_valid_stops() {
        // On "abcd " @ vw 4 the phantom row (row 1) is empty. A click/overshoot on it resolves to eol
        // (empty-row branch, not a teleport); move_home/move_end on it land on a valid stop; down onto
        // it then up preserves the source offset (Law 5 shape).
        let (_rows, map) = layout("abcd ", BlockRole::Paragraph, LineRender::Concealed, 4, false, 0);
        assert_eq!(map.visual_to_source(1, 0), map.eol);
        assert_eq!(map.visual_to_source(1, 9), map.eol, "overshoot clamps to eol, no teleport");
        let probe = Cursor { offset: map.visual_to_source(1, 0), row: 1, desired_col: 0 };
        assert!(map.is_cursor_stop(move_home(&map, probe).offset));
        assert!(map.is_cursor_stop(move_end(&map, probe).offset));
        let down = move_down_within(&map, Cursor { offset: 0, row: 0, desired_col: 0 }).expect("row 1");
        assert_eq!(down.row, 1);
        assert_eq!(move_up_within(&map, down).expect("back to row 0").offset, 0);
    }
```

### 1d. Update the in-code "spec D2" comments

Point every "spec D2" reference at the recorded amendment (deliberate recorded act per CLAUDE.md).
Substantive edits where the comment describes the HANG; a light amendment-pointer where it describes the
(unchanged) re-placement/repeat logic:

- `layout.rs:304` `// Word-boundary soft-wrap (UAX #14; spec D1/D2). CodeBlock keeps grapheme wrap.` →
  append `` B17 amends D2's prose hang (empty flush row at the margin; see the B17 design note).``
- `layout.rs:322-325` (the `is_ws`/`hang` block): update the hang comment to note that at the margin the
  hang is now paired with an appended flush row (§1a) for non-CodeBlock roles.
- `layout.rs:326` and `:350` ("spec D2 as amended" — the repeat/re-place logic, semantically UNCHANGED):
  change "spec D2 as amended" → "spec D2 (prose hang further amended by B17 — see the design note)".
  Do NOT alter the re-place logic.
- `layout.rs:713` test comment ("byte-identical to today (spec D2 as amended)"): unchanged behavior;
  same amendment-pointer touch.

### 1e. Rewrite the render golden + clamp comment (same commit — keeps the workspace green)

Replace `hung_trailing_space_caret_pins_at_edge` (`render.rs:2005-2019`) with:

```rust
    #[test]
    fn trailing_space_at_margin_breaks_caret_to_flush_row() {
        // B17 (amends spec D2): vw 4, "abcd " — the caret AFTER the hung space (byte 5, eol) no longer
        // pins at the edge; it moves to col 0 of the next (flush phantom) screen row — real line-move
        // feedback. (Pre-B17 this pinned at x == 3 on the same row.)
        let mut e = Editor::new_from_text("abcd \n", None, (4, 8));
        set_caret(&mut e, 5); // eol, after the hung space (active line — layout raw)
        derive::rebuild(&mut e);
        assert_eq!(crate::nav::screen_pos(&e), Some((0, 1)), "caret at col 0 of the flush row");
        let cur = render_capturing_cursor(&mut e, 4, 8);
        assert_eq!(cur, Some((0, 1)), "hardware caret at (0, 1) — col 0, one row down");
    }
```

Update the clamp comment at `render.rs:453-454` (the `else if !editor.has_active_input_overlay()` arm);
leave the clamp expression itself unchanged — only the comment:

```rust
            // Guard rows; clamp cols. B17 (amends spec D2): a trailing space at the margin now resolves
            // to col 0 of the flush continuation row, so this clamp is inert for that case. It still
            // guards a caret placed on a hung-INTERIOR cell (before the space) and a genuinely over-wide
            // single grapheme past the rect.
            if row < edit_height && tg.text_width > 0 {
                let col = col.min((tg.text_width as usize).saturating_sub(1) as u16);
```

### 1f. CONFIRM the laws (a run, not an assertion)

Run `cargo test -p wordcartel-core props` (or `cargo test -p wordcartel-core layout`) and state in the
commit/report that all six laws (`law1_colmap_roundtrip`, `law2_no_cursor_in_conceal`,
`law3_softwrap_fidelity`, `law_w1_no_needless_midword_break`, `law4_active_identity`,
`law5_desired_col_preserved`, `law6_all_nav_ops_land_on_stop_concealed`) pass **unmodified**. If any
reddens, the epilogue perturbed `placed` — it must not; fix the epilogue, not the law.

**Verify Task 1:** `cargo test -p wordcartel-core` green AND `cargo test --workspace` green (the render
golden is rewritten in this commit, so the workspace is green as committed);
`cargo clippy -p wordcartel-core --all-targets` clean; property laws confirmed by the run. Commit.

---

## Task 2 — Additive shell tests: nav, B10, ventilate (`wordcartel` crate)

**Deliverable:** three NEW (purely additive) tests — a `screen_pos` End-on-hung-row → flush-row test, the
B10 EOF-stack test, and the ventilate negative guard. All new; none can break existing greenness.

**Interfaces (real signatures — do not change):**
- `pub fn screen_pos(editor: &Editor) -> Option<(u16, u16)>` — `nav.rs:83` (resolves the visible caret from the head offset via `source_to_visual`).
- `pub fn caret_line(editor: &Editor) -> usize` — `nav.rs:45` (B10: `byte_to_line(h.min(buf.len()))`, `:53`).
- `pub fn move_end(editor: &mut Editor) -> usize` — `nav.rs:255` (returns the new offset; does not move the caret).
- `pub fn segment_block(block_text: &str) -> impl Iterator<Item = (usize, usize)> + '_` — `ventilate.rs:73`; `pub fn sentence_display(raw_span: &str) -> String` — `ventilate.rs:57`.
- Test helper in `nav.rs mod tests`: `set_caret(&mut Editor, usize)` `:1052`. `Editor::new_from_text(&str, Option<..>, (u16,u16))`.

### 2a. Nav tests (append to `nav.rs mod tests`)

```rust
    #[test]
    fn end_on_hung_row_resolves_caret_to_flush_row() {
        // move_end on the hung row 0 returns the eol offset; screen_pos then resolves that offset to the
        // flush continuation row (screen_pos reads the offset, not the Cursor row affinity).
        let mut e = Editor::new_from_text("abcd \n", None, (4, 8));
        set_caret(&mut e, 0);
        derive::rebuild(&mut e);
        let end = crate::nav::move_end(&mut e);
        set_caret(&mut e, end);
        derive::rebuild(&mut e);
        assert_eq!(crate::nav::screen_pos(&e), Some((0, 1)),
            "End lands the visible caret on the flush phantom row");
    }

    #[test]
    fn trailing_space_before_eof_stacks_flush_row_above_b10_phantom_line() {
        // "abcd \n" with the caret at EOF: B17's flush VISUAL row (line 0) and B10's phantom LOGICAL
        // line (the trailing empty line) coexist, stacked. The caret sits on the EOF logical line,
        // below line 0's content row AND its flush row.
        let mut e = Editor::new_from_text("abcd \n", None, (4, 8));
        let eof = e.active().document.buffer.len(); // 6 = start of the phantom logical line
        set_caret(&mut e, eof);
        derive::rebuild(&mut e);
        assert_eq!(crate::nav::caret_line(&e), 1, "B10: EOF caret on the phantom logical line, not glued");
        assert_eq!(crate::nav::screen_pos(&e), Some((0, 2)),
            "EOF caret below line 0's content row AND its flush phantom row");
    }
```

### 2b. Ventilate negative guard (append to `ventilate.rs mod tests`)

```rust
    #[test]
    fn ventilated_sentence_display_never_ends_in_whitespace() {
        // §7 (fork e moot): sentence_spans is content-only — no trailing whitespace (textobj.rs:190-192,
        // content_end trims via trim_end_matches) — and sentence_display only maps INTERIOR '\n'→space.
        // So no ventilated sentence display can end in a space/tab, and B17's trailing-space trigger
        // NEVER fires under the lens (no phantom flush row mid- or end-of-block). Guards the trim
        // invariant that makes fork (e) a non-event.
        let block = "One two three.  Four\nfive six.   Seven.  ";
        for (sf, st) in super::segment_block(block) {
            let display = super::sentence_display(&block[sf..st]);
            assert!(!display.ends_with(' ') && !display.ends_with('\t'),
                "sentence display {display:?} ends in whitespace — a phantom row could leak under the lens");
        }
    }
```

**Verify Task 2:** `cargo test -p wordcartel` green; `cargo clippy -p wordcartel --all-targets` clean.
Run the full workspace once (`cargo test`, `cargo clippy --workspace --all-targets`) and the PTY smoke
suite (`scripts/smoke/run.sh`, quote its one-line summary). Commit.

---

## Sequencing

Task 1 (core + the render-golden rewrite) is a strict prerequisite: it introduces the phantom row that
Task 2's additive tests observe. Task 1 is **workspace-green as committed** — the epilogue's only
cross-crate casualty is the render golden `hung_trailing_space_caret_pins_at_edge`, which Task 1 rewrites
in the same commit (Codex verified it is the sole red test; no e2e/nav wrap journey shares that
assertion). Task 2 is **purely additive** — three NEW tests against the already-landed behavior, so it
cannot break greenness. The property-law CONFIRMATION happens at Task 1 (a run). Final gates (opus
whole-branch review + Codex pre-merge GO/NO-GO) run after both tasks land.
