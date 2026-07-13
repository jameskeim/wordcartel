# S6 — Ventilate-as-a-lens: implementation PLAN

> **For agentic workers:** REQUIRED SUB-SKILL: use `superpowers:subagent-driven-development` (recommended)
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`)
> syntax for tracking.

**Goal:** Add a non-destructive, per-buffer *ventilate lens* — toggle it on and paragraph prose
redraws one sentence per visual row-group with a left word-count gutter; toggle it off and the buffer
is byte-identical — so the sentence you SEE segmented is exactly the sentence `select-sentence`
SELECTS (SEE==SELECT).

**Architecture:** A layout lens (the `measure` precedent), NOT a fifth `RenderMode`. A new per-buffer
`View.ventilate: bool` + `LayoutKey.ventilate` field gate a **window-scoped layout path** in a new
cohesive module (`wordcartel/src/ventilate.rs`): classify each block via `role_at` (Paragraph = prose,
everything else = verbatim), take each prose block's window `(ps, pe) = nav::paragraph_range_at(...)`
— **the identical call `select-sentence` and focus make**, so the lens and the selector cannot
diverge — gather `buf.slice(ps..pe)`, segment the **RAW** text with
`wordcartel_core::textobj::sentence_spans` (so the semantic-hard-break veto governs the view
identically to selection), and emit each sentence as a soft-wrapped row-group via the existing
`layout()` engine at a gutter-reduced width. A shared **window-aware resolver** (line-index LOOKUP +
`ps` byte-ORIGIN, `ps` = `paragraph_range_at` start) is the single seam every nav AND render consumer
takes its origin from — so SEE==SELECT holds by construction for indented / hard-wrapped / gap-fallback
paragraphs alike. Command surface: a `register_stateful` `toggle_ventilate` row + one shared setter, `alt-v` in
CUA.

**Tech Stack:** Rust 2021, functional-core (`wordcartel-core`, `#![forbid(unsafe_code)]`, pure) +
imperative-shell (`wordcartel`, ratatui 0.30 + crossterm). Reuses `layout::layout`,
`textobj::sentence_spans`, `count::word_count`, the `BlockTree`, and the existing `derive::rebuild`
cache pipeline. No new dependency; **`wordcartel-core` gains NO `repar` dependency**.

**Authority:** the Codex-clean spec — `docs/superpowers/specs/2026-07-13-s6-ventilate-lens-design.md`.
Section references below (§3, §5.2, …) point at that spec. The spec governs *why*; this plan governs
*how*. A finding that contradicts an approved decision (F1–F5 / L1–L7) is a HUMAN decision, never a
silent fix.

**Grounding:** every signature, field, and anchor below was re-verified against the real source
2026-07-13 while authoring. Anchors are symbol-anchored; if a line number has drifted, locate by the
named symbol (`documentSymbol`/`grep`), never trust the number (recurring S5 lesson).

---

## Global Constraints

Every task's requirements implicitly include this section.

1. **The Codex-clean spec is the authority.** Contradiction → human decision, not a silent fix.
   Approved decisions that MUST NOT change: F1 (layout lens, not RenderMode), F2 (paragraph reflow
   across hard newlines), F3 (6-col word-count gutter), F4 (paragraphs ONLY; blockquotes deferred),
   F5 (per-buffer state on `View`), L1 (no raw reveal on ventilated PROSE rows — verbatim lines keep
   the active-line raw reveal, `line_render_for(mode, l == active_line)`), L2, L3 (perf), L4
   (giant-block accepted, no cap), L5 (no `Command` enum variant), L6 (naming/binding), L7 (`999`
   clamp, `count::word_count`).
2. **`wordcartel-core` is pure.** `#![forbid(unsafe_code)]`, **NO `repar` dependency**, no shell
   types. The only core change is an additive parameter on `layout()` (Task 5); the ventilate module,
   its state, and all reflow logic live in the **SHELL** (`wordcartel/src/ventilate.rs`).
3. **SEE==SELECT is the headline invariant.** The lens's gather window AND origin are
   `nav::paragraph_range_at`'s `(ps, pe)` / `ps` — the IDENTICAL call `select-sentence` and focus
   make. Segment the RAW window (byte-identical to `sentence_spans` on raw text); normalize interior
   `\n`→space ONLY in each already-segmented span's DISPLAY string (byte-length-preserving). The
   window-aware resolver supplies the `ps` ORIGIN to every nav AND render consumer; `line_start(l)`
   is used for neither lookup nor offset in the ventilated path.
4. **Byte-identical toggle.** Bytes never change; toggle-off restores the buffer byte-for-byte and
   preserves the caret. The lens has NO edit path.
5. **Per-buffer state on `View`** (F5), default off. NOT a `SettingsSnapshot` field, NOT a config key
   (like `View.mode`) — so the Law-2 persisted-setting guard is N/A to it.
6. **House style — hand-formatted.** Do **NOT** run `cargo fmt` (no `rustfmt.toml`). Match dense
   neighbor style by hand: 4-space indent, `—` em-dashes in prose comments (never `--`), imports
   grouped by hand. **No emoji anywhere except the multibyte TEST fixtures** (`é`/`中`/`🙂`).
   Doc-comment every new public item (params/returns; `# Examples` for the non-obvious ones).
7. **Merge GATEs (every task leaves ALL green — the tree compiles + passes after each task):**
   - `cargo test --workspace` green (core lib + oracle, shell lib, shell integration tests).
   - `cargo build` and `cargo test --no-run` warning-free for touched crates.
   - `cargo clippy --workspace --all-targets` clean (`[workspace.lints.clippy] all = "deny"`).
   - `wordcartel/tests/module_budgets.rs` 5/5 (`app.rs` 1000, `render.rs` **900**, `timers.rs` 400,
     `plugin/host.rs` 400, `plugin/pump.rs` 350). `derive.rs` is NOT budgeted, but the ventilate
     branch there stays THIN (delegates into `ventilate.rs`); the gutter paint must not push
     `render.rs` over 900 (keep the paint helper small / in `ventilate.rs`).
   - `clippy::too_many_lines` = 100 (`clippy.toml`): factor every new fn under 100 lines; an
     unavoidable flat dispatch carries an item-local `#[allow(clippy::too_many_lines)]` with a reason.
   - Command-surface invariants (Task 7): `palette_is_exhaustive_over_the_registry` (`palette.rs`),
     `hints_reresolve_on_preset_switch` (`keymap.rs`), `custom_bind_surfaces_in_menu_and_palette`
     (`menu.rs`) — stay green.
   - Backlog bijection (`wordcartel/tests/backlog.rs`) untouched (this effort ships via normal merge;
     backlog status flipped separately at merge — do NOT hand-edit `BACKLOG.md`/`backlog.toml`).
8. **PTY smoke suite** (`scripts/smoke/run.sh`): mandatory-run / advisory-pass — quote its one-line
   summary in the pre-merge report; a red result is advisory, never a merge blocker.
9. **Commit trailers (verbatim) on every commit** — append after the one-line subject:
   ```
   Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
   Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG
   ```

**Anchor table (verified 2026-07-13):**

| Symbol | Location |
|---|---|
| `struct View` (add `ventilate`, `vent_blocks`) | `wordcartel/src/editor.rs:111-119`; ctor `editor.rs:204-210`; default `mode` @ 208 |
| `Editor::invalidate_layout` (clears `line_layouts` + `layout_key`) | `editor.rs:332-335` |
| `struct LayoutKey` (add `ventilate`) | `wordcartel/src/derive.rs:11-22`; ctor `derive.rs:232-242`; gate `derive.rs:243-245` |
| `derive::rebuild_downstream` fill loop (thin branch here) | `derive.rs:257-273` (per-line `layout::layout` @ 267, insert @ 269) |
| `vp_width` (gutter width base) | `derive.rs:230` (`nav::text_geometry(editor).text_width as usize`) |
| `layout::layout` (add `reserve_cols`) | `wordcartel-core/src/layout.rs:244-250`; `prefix_width` init @ 284-297, `col = prefix_width` @ 308 |
| `struct VisualRow` / `struct ColMap` (`prefix_width`, `src_span`, `eol`) | `layout.rs:45-58` / `60-82` |
| `textobj::sentence_spans` | `wordcartel-core/src/textobj.rs:202` |
| `count::word_count` | `wordcartel-core/src/count.rs:6` |
| `BlockTree` / `Block{kind,span}` / `BlockKind::Paragraph` / `top_level` / `role_at` | `wordcartel-core/src/block_tree.rs:204/191-199/160/211/228` |
| `BlockRole` (`Paragraph` variant) | `wordcartel-core/src/style.rs` (via `role_precedence`, `block_tree.rs:236-251`) |
| `nav::paragraph_range_at` / `deepest_block_at` | `wordcartel/src/nav.rs:655` / `640` |
| `nav::text_geometry` → `TextGeometry{text_left,text_width}` | `nav.rs:18-37` |
| `nav::get_or_layout` / `layout_line_on_demand` / `layout_line_active` | `nav.rs:153` / `60` / `142` |
| `nav::screen_pos` / `clamp_snap` / `rows_before_caret` | `nav.rs:82` / `163` / `532` |
| `render::gather_row_ctx` (focus region, search window) | `wordcartel/src/render.rs:488-575` (focus @ 494-512, search window @ 530-533) |
| `render::paint_rows` (focus-dim origin) | `render.rs:744-791` (`line_off` @ 759, `g_from/g_to` @ 760-761) |
| `render::row_spans_placed` (diag/search/sel origin) | `render.rs:649-665` (`line_off` @ 652, `lo/hi` @ 657-658) |
| `registry` `register_stateful` / `MenuMark` / `toggle_measure` row | `registry.rs:118` / `82` / `553-555` |
| CUA keymap table `static CUA` (`alt-v` FREE) | `wordcartel/src/keymap.rs:257`; `build_keymap`/`parse_seq`/`Resolution`/`resolve` @ 496/109/148/210 |
| e2e `Harness` (`new`, `render`, `row`, `dim_cols`, `underlined_cols`, `alt`) | `wordcartel/src/e2e.rs:75/197/244/265/258/205` |
| registry test harness (`Ctx`, `dispatch`, `Z` clock, `InlineExecutor`) | `registry.rs:26-31/764/973-974/970` |
| `TextBuffer::slice/len/byte_to_line/is_empty` | `wordcartel-core/src/buffer.rs:125/33/140/38` |

---

## Task list

| # | Title | Crate | Seam it touches |
|---|---|---|---|
| 1 | Per-buffer `View.ventilate` + `LayoutKey.ventilate` (no behavior; keyed into cache) | wordcartel | `View`/`LayoutKey` + cache gate |
| 2 | Ventilate module core: block classify + raw gather + segment-raw/display-normalize split | wordcartel | new cohesive module (pure fns) |
| 3 | Window-aware resolver (line-index lookup + `ps` = `paragraph_range_at` start origin) + `View.vent_blocks` | wordcartel | the SEE==SELECT keystone |
| 4 | Migrate ALL consumers (nav + render) to the resolver origin | wordcartel | nav on-demand + render paint sites |
| 5 | Lens layout path wired into `derive::rebuild` (thin branch) + `layout()` `reserve_cols` | wordcartel-core + wordcartel | `derive` fill branch; core layout param |
| 6 | 6-col rhythm gutter render (word count, `999` clamp, continuation `│`, verbatim reserve) | wordcartel | render paint (gutter cells) |
| 7 | Command surface: `toggle_ventilate` + shared setter + `alt-v` + contract gates | wordcartel | registry + CUA keymap |
| 8 | Integration/e2e + guardrails (SEE==SELECT, idempotence, paint-origin, perf) | wordcartel | test-only |

Dependency order: 1 → 2 → 3 → 4 → 5 → 6 → 7, with 8 last (depends on 1–7). Each task is independently
green.

---

## Task 1 — Per-buffer `View.ventilate` + `LayoutKey.ventilate`

**Files:**
- Modify: `wordcartel/src/editor.rs` (`View` struct + ctor)
- Modify: `wordcartel/src/derive.rs` (`LayoutKey` struct + ctor + a re-run test case)

**Command-surface conformance:** N/A — adds a field, no command yet (that is Task 7).

**Interfaces:**
- Consumes: nothing.
- Produces: `View.ventilate: bool` (default `false`); `LayoutKey.ventilate: bool` (set from
  `view.ventilate`). Both consumed by every later task.

- [ ] **Step 1: Write the failing test** — add to `derive.rs` `#[cfg(test)] mod tests`, extending the
  `layout_gate_reruns_on_each_input` macro cases (near `derive.rs:740`) with a `ventilate` case:

```rust
    #[test]
    fn ventilate_flag_reruns_layout() {
        // Flipping view.ventilate must change LayoutKey → the gate misses → the fill re-runs.
        let mut e = Editor::new_from_text("Hello there. Bye now.\n", None, (80, 24));
        LAYOUT_RUNS.with(|c| c.set(0));
        rebuild(&mut e);
        let before = LAYOUT_RUNS.with(|c| c.get());
        e.active_mut().view.ventilate = true;
        rebuild(&mut e);
        let after = LAYOUT_RUNS.with(|c| c.get());
        assert!(after > before, "toggling ventilate must re-run the layout loop");
        // Default is off.
        let e2 = Editor::new_from_text("x\n", None, (80, 24));
        assert!(!e2.active().view.ventilate, "ventilate defaults OFF");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel ventilate_flag_reruns_layout`
Expected: FAIL — `no field 'ventilate' on type 'View'` (compile error).

- [ ] **Step 3: Add the `View` field + ctor default.** In `editor.rs`, add to `struct View`
  (`editor.rs:111-119`) after `mode`:

```rust
    pub mode: RenderMode,
    /// S6 — per-buffer ventilate lens (sentence-per-line + rhythm gutter). Default off; a lens is
    /// into THIS writing, so it does not follow other buffers (§F5). Keyed into `LayoutKey`.
    pub ventilate: bool,
    /// Per-visible-logical-line layout cache (Task 3).
    /// Key = logical line index; value = (visual rows, source↔visual ColMap).
    pub line_layouts: BTreeMap<usize, (Vec<VisualRow>, ColMap)>,
```

  And in the `View { … }` ctor (`editor.rs:204-210`) after `mode: RenderMode::LivePreview,`:

```rust
            mode: RenderMode::LivePreview,
            ventilate: false,
            line_layouts: BTreeMap::new(),
```

- [ ] **Step 4: Add the `LayoutKey` field + ctor.** In `derive.rs`, add to `struct LayoutKey`
  (`derive.rs:11-22`) after `mode`:

```rust
    pub mode: crate::editor::RenderMode, // view.mode — drives per-line LineRender
    pub ventilate: bool,                 // S6 — view.ventilate; sentence-per-line layout path
    pub heading_level_glyph: bool,
```

  And in the `LayoutKey { … }` construction (`derive.rs:232-242`) after `mode: b_mode,`. Note
  `b_mode` is snapshotted at `derive.rs:220` (`let b_mode = b.view.mode;`) — add a sibling snapshot
  `let b_ventilate = b.view.ventilate;` in that same `let (…) = { … }` block (extend the tuple), then:

```rust
        mode: b_mode,
        ventilate: b_ventilate,
        heading_level_glyph: editor.theme.heading_level_glyph,
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p wordcartel ventilate_flag_reruns_layout`
Expected: PASS.

- [ ] **Step 6: Full gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets && cargo test -p wordcartel --test module_budgets`
Expected: all green; module_budgets 5/5.

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/editor.rs wordcartel/src/derive.rs
git commit -m "feat(editor): per-buffer View.ventilate + LayoutKey.ventilate cache field (S6)"
# + trailers
```

---

## Task 2 — Ventilate module core: classify + raw gather + segment-raw/display-normalize split

**Files:**
- Create: `wordcartel/src/ventilate.rs`
- Modify: `wordcartel/src/main.rs` or the crate root (`wordcartel/src/lib.rs` if present) to add
  `mod ventilate;` (locate the existing `mod derive;` / `mod nav;` declarations and add beside them).

**Command-surface conformance:** N/A — pure helpers, no command surface.

**Interfaces:**
- Consumes: `wordcartel_core::textobj::sentence_spans` (`textobj.rs:202`);
  `wordcartel_core::block_tree::{BlockTree}`; `wordcartel::nav::paragraph_range_at`;
  `TextBuffer::slice`.
- Produces:
  - `pub const GUTTER_COLS: usize = 6;`
  - `pub const GUTTER_MAX: u16 = 999;`
  - `pub fn prose_block_at(blocks: &BlockTree, buf: &TextBuffer, line_start_byte: usize) -> Option<(usize, usize)>`
    — `Some((span_start, span_end))` iff the block containing `line_start_byte` is a `Paragraph`
    (prose); `None` for verbatim blocks.
  - `pub fn sentence_display(raw_span: &str) -> String` — the DISPLAY string of one already-segmented
    sentence: interior `\n` → single space, byte-length-preserving. (Used by the fill, Task 5.)
  - `pub fn segment_block(block_text: &str) -> impl Iterator<Item = (usize, usize)> + '_` — the RAW
    sentence spans of a gathered window (a thin re-export of `sentence_spans`, named for intent so the
    fill and tests read as "segment the raw window"). Offsets are window-relative — into `block_text`
    (= `buf.slice(ps..pe)`), so the global byte is `ps + offset`.

- [ ] **Step 1: Write the failing test** — create `wordcartel/src/ventilate.rs` with only a
  `#[cfg(test)]` module first (so the test compiles against the yet-unwritten fns and fails):

```rust
//! S6 — the ventilate lens: non-destructive sentence-per-line layout of paragraph prose.
//! Pure classification/gather/segment helpers here; the cache wiring is Task 3/5, the gutter
//! render Task 6. The lens SEGMENTS THE RAW block text (so the semantic-hard-break veto governs
//! the view identically to `select-sentence`) and normalizes interior `\n`→space ONLY in each
//! span's DISPLAY string (byte-length-preserving — ColMap `src` offsets stay valid). §5.1.

use wordcartel_core::block_tree::BlockTree;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::textobj::sentence_spans;

// … public items land in Step 3 …

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Editor;

    #[test]
    fn classify_paragraph_vs_verbatim() {
        // A paragraph, a heading, and a fenced code block.
        let e = Editor::new_from_text("Para one. Para two.\n\n# Heading\n\n```\ncode\n```\n", None, (80, 24));
        let buf = &e.active().document.buffer;
        let blocks = e.active().document.blocks();
        // Byte 0 is inside the paragraph → Some(span covering "Para one. Para two.").
        let p = prose_block_at(blocks, buf, 0).expect("paragraph is prose");
        assert_eq!(buf.slice(p.0..p.1), "Para one. Para two.");
        // The heading line start → None (verbatim).
        let h_start = buf.slice(0..buf.len()).find("# Heading").unwrap();
        assert!(prose_block_at(blocks, buf, h_start).is_none(), "heading is verbatim");
        // Inside the code fence → None (verbatim).
        let c_start = buf.slice(0..buf.len()).find("code").unwrap();
        assert!(prose_block_at(blocks, buf, c_start).is_none(), "code block is verbatim");
    }

    #[test]
    fn segment_raw_preserves_hard_break_veto() {
        // A two-space hard break (verse) must remain TWO sentences — the RAW text carries the
        // "  \n" the veto reads. Stripping \n first would merge them (SEE≠SELECT).
        let raw = "Roses are red,  \nViolets are blue.";
        assert_eq!(segment_block(raw).count(), 2, "hard-break veto keeps two spans on RAW text");
        // A soft wrap (single trailing space) merges to one.
        let soft = "The soft wrap ends here \nand continues on.";
        assert_eq!(segment_block(soft).count(), 1);
    }

    #[test]
    fn display_normalizes_newline_length_preserving() {
        let raw = "The committee met\nand voted."; // one soft-wrapped sentence
        let disp = sentence_display(raw);
        assert_eq!(disp, "The committee met and voted."); // \n → single space
        assert_eq!(disp.len(), raw.len(), "byte-length-preserving (\\n and space are both 1 byte)");
        assert!(!disp.contains('\n'));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel ventilate::tests`
Expected: FAIL — `cannot find function 'prose_block_at'` etc.

- [ ] **Step 3: Write the implementation** — insert above the `#[cfg(test)]` module:

```rust
/// Columns reserved on the left for the rhythm gutter: `NNN │ ` (3-digit count, space, rule,
/// space). A fixed reservation subtracted from the wrap width (§3.4) and painted by render (Task 6).
pub const GUTTER_COLS: usize = 6;

/// The 3-digit gutter saturates here — a ≥1000-word "sentence" is not real prose (§7, L7).
pub const GUTTER_MAX: u16 = 999;

/// `Some((ps, pe))` — the WINDOW of the prose block containing `line_start_byte`, iff it is PROSE
/// (a Markdown paragraph); `None` for every verbatim block (heading, list, code, table, thematic
/// break, and — S6 — blockquote, F4/L2). The window is `nav::paragraph_range_at`'s return — **the
/// IDENTICAL call `select-sentence` (`commands.rs` `Scope::Sentence`) and focus-Sentence
/// (`render.rs:503`) make** — so `ps` is the gather/segment origin the selector uses, and SEE==SELECT
/// + focus-window-identity hold by construction (indented, hard-wrapped, AND gap-fallback cases;
/// §5.2/§6.4). The block tree's `role_at` is used ONLY to CLASSIFY prose vs verbatim; the WINDOW and
/// ORIGIN are `paragraph_range_at`'s — NEVER `block.span.start` (which diverges from `ps` on the
/// physical `line_start`-based gap fallback, `nav.rs:662-685`).
pub fn prose_block_at(blocks: &BlockTree, buf: &TextBuffer, line_start_byte: usize) -> Option<(usize, usize)> {
    if blocks.role_at(line_start_byte) != wordcartel_core::style::BlockRole::Paragraph {
        return None;
    }
    Some(crate::nav::paragraph_range_at(blocks, buf, line_start_byte))
}

/// The DISPLAY string of one already-segmented sentence span: interior `\n` (the author's hard
/// newlines) → a single space, so `layout()` (which treats its input as ONE logical line) wraps it
/// as flowing prose. **Byte-length-preserving** — `\n` and `' '` are both one byte, so every
/// resulting `ColMap.src` offset still indexes the live buffer (§5.1). This is the ONLY permitted
/// normalization, and it runs AFTER segmentation (never before — that would defeat the
/// hard-break veto, §5.1).
pub fn sentence_display(raw_span: &str) -> String {
    raw_span.replace('\n', " ")
}

/// The RAW sentence spans of a gathered window (offsets window-relative to `ps`). A thin,
/// intent-named re-export of `sentence_spans`: the lens segments the RAW window text so the semantic-hard-break
/// veto governs the view identically to `select-sentence` (§5.1, §3.3 step 2).
pub fn segment_block(block_text: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    sentence_spans(block_text)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p wordcartel ventilate::tests`
Expected: PASS (3 tests).

- [ ] **Step 5: Full gates + doc test**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets`
Expected: green. (`String::replace` allocates once per sentence display — acceptable: the ventilate
fill is a cold, summoned-view path, not the per-keystroke hot path; §4.3.)

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/ventilate.rs wordcartel/src/main.rs
git commit -m "feat(ventilate): block classify + raw-segment/display-normalize helpers (S6)"
# + trailers
```

---

## Task 3 — Block-aware resolver + `View.vent_blocks` (the SEE==SELECT keystone)

**Files:**
- Modify: `wordcartel/src/ventilate.rs` (add `VentBlock`, `GutterCell`, `Resolved`, `resolve`)
- Modify: `wordcartel/src/editor.rs` (add `View.vent_blocks`; clear it in `invalidate_layout`)

**Command-surface conformance:** N/A.

**Interfaces:**
- Consumes: `View.line_layouts` (`editor.rs:118`); `TextBuffer::byte_to_line` (`buffer.rs:140`),
  `line_to_byte` (`buffer.rs:145`).
- Produces:
  - `pub enum GutterCell { Count(u16), Continuation }` (paragraph gutter cells; filled Task 6).
  - `pub struct VentBlock { pub last_line: usize, pub byte_origin: usize, pub gutter: Vec<GutterCell> }`
    — per ventilated-paragraph metadata, keyed in `View.vent_blocks` by the window's FIRST logical
    line (the same key the block's `line_layouts` entry uses). `last_line` = the window's last content
    line (LOOKUP range end); `byte_origin` = `ps` = `paragraph_range_at`'s window start (OFFSET origin
    — the selector's origin); `gutter` = one cell per `VisualRow` in the entry (empty until Task 6).
  - `pub struct View.vent_blocks: BTreeMap<usize, VentBlock>` — empty when ventilate off.
  - `pub struct Resolved<'a> { pub rows: &'a [VisualRow], pub map: &'a ColMap, pub byte_origin: usize, pub first_line: usize, pub last_line: usize }`
  - `pub fn resolve<'a>(view: &'a crate::editor::View, buf: &TextBuffer, l: usize) -> Option<Resolved<'a>>`
    — the shared window-aware lookup: line-index range membership, `ps` origin. `None`
    when line `l` is not covered by any cached entry (caller falls back to on-demand, Task 4).
  - `pub fn vent_block_range(buf: &TextBuffer, ps: usize, pe: usize) -> (usize, usize)`
    — `(first_line, last_line)` for a window `[ps, pe)` (helper the fill and resolver share).

- [ ] **Step 1: Write the failing test** — add to `ventilate.rs` tests. **T-indent-origin** is the
  keystone: the resolver must RESOLVE for an interior line of an INDENTED paragraph, the origin must
  be `ps` (`paragraph_range_at` start), and — the SEE==SELECT proof — the lens's global sentence spans
  must EQUAL what `select-sentence` selects.

```rust
    #[test]
    fn resolver_resolves_interior_line_and_origin_is_line_start_when_off() {
        // Ordinary (non-ventilated) per-line entry: keyed exactly at l, origin == line_start.
        let mut e = Editor::new_from_text("alpha\nbeta\ngamma\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let buf = e.active().document.buffer.clone();
        let view = &e.active().view;
        let r = resolve(view, &buf, 1).expect("per-line entry for line 1 resolves");
        assert_eq!(r.byte_origin, buf.line_to_byte(1), "per-line origin is line_start");
        assert_eq!(r.first_line, 1);
        assert_eq!(r.last_line, 1);
    }

    #[test]
    fn t_indent_origin_lens_spans_equal_select_sentence_for_indented_paragraph() {
        // A 2-space-INDENTED, multi-line paragraph. paragraph_range_at's ps is AFTER the two spaces,
        // so a byte-containment test against line_start(anchor) would FAIL; line-index membership must
        // succeed. The origin must be ps, and the lens's global sentence spans must be byte-identical
        // to what select-sentence selects (the SEE==SELECT proof on the indent case).
        let text = "  The committee met on a\nsunny Tuesday afternoon. It voted.\n";
        let mut e = Editor::new_from_text(text, None, (30, 24));
        e.active_mut().view.ventilate = true;
        crate::derive::rebuild(&mut e); // Task 5 fill populates vent_blocks for the paragraph
        let buf = e.active().document.buffer.clone();
        let blocks = e.active().document.blocks().clone();
        // Line 1 ("sunny Tuesday…") is an INTERIOR line of the window (anchor is line 0).
        let r = resolve(&e.active().view, &buf, 1).expect("interior line of the ventilated window RESOLVES");
        assert_eq!(r.first_line, 0, "resolves to the window anchor");
        assert!(r.last_line >= 1, "range covers the interior line");
        // Origin == ps == paragraph_range_at start (after the 2-space indent), NOT line_start(anchor).
        let (ps, pe) = crate::nav::paragraph_range_at(&blocks, &buf, 0);
        assert_eq!(r.byte_origin, ps, "origin is ps (paragraph_range_at start)");
        assert_ne!(r.byte_origin, buf.line_to_byte(0), "origin is NOT line_start(anchor) — the indent delta");
        // SEE==SELECT: the lens's global sentence spans == sentence_spans over the SAME window select
        // uses. For each, select-sentence with the caret inside must return the identical span.
        let win = buf.slice(ps..pe);
        let lens_spans: Vec<(usize, usize)> =
            crate::ventilate::segment_block(&win).map(|(sf, st)| (ps + sf, ps + st)).collect();
        for &(gf, gt) in &lens_spans {
            // select-sentence uses scope_range_at over paragraph_range_at + sentence_bounds — identical
            // window + origin — so the selected span equals the lens span for a caret inside it.
            let (sf, st) = wordcartel_core::textobj::sentence_bounds(&win, ((gf + gt) / 2) - ps);
            assert_eq!((ps + sf, ps + st), (gf, gt), "lens span EQUALS select-sentence span (SEE==SELECT)");
        }
    }
```

> This test depends on the Task 5 fill to populate `vent_blocks`. Write it now (RED), implement the
> resolver + `vent_blocks` in this task, and it goes GREEN once Task 5 lands. If executing strictly
> task-by-task, gate the SECOND test `#[ignore = "needs Task 5 fill"]` until Task 5, then un-ignore;
> the FIRST test (per-line, no ventilate) passes at the end of this task. Note the choice in the
> commit message.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel resolver_resolves_interior_line`
Expected: FAIL — `cannot find function 'resolve'` / `no field 'vent_blocks'`.

- [ ] **Step 3: Add `View.vent_blocks` + clear it on invalidation.** In `editor.rs` `struct View`
  after `line_layouts`:

```rust
    pub line_layouts: BTreeMap<usize, (Vec<VisualRow>, ColMap)>,
    /// S6 — per ventilated-PARAGRAPH metadata, keyed by the window's FIRST logical line (the same
    /// key its `line_layouts` entry uses). Empty when `ventilate` is off. The shared resolver
    /// (`ventilate::resolve`) reads this to map any interior line to its window anchor (line-index
    /// LOOKUP) and to supply the `ps` (`paragraph_range_at` start) byte ORIGIN. Verbatim blocks get
    /// NO entry.
    pub vent_blocks: BTreeMap<usize, crate::ventilate::VentBlock>,
```

  In the ctor (`editor.rs:204-210`) after `ventilate: false,`:

```rust
            ventilate: false,
            line_layouts: BTreeMap::new(),
            vent_blocks: BTreeMap::new(),
```

  In `invalidate_layout` (`editor.rs:332-335`) add the clear beside `line_layouts.clear()`:

```rust
    pub fn invalidate_layout(&mut self) {
        self.view.line_layouts.clear();
        self.view.vent_blocks.clear();
        self.layout_key = None;
    }
```

- [ ] **Step 4: Add the resolver types + fn** to `ventilate.rs` (above the tests). Import
  `VisualRow`, `ColMap`, `View`:

```rust
use wordcartel_core::layout::{ColMap, VisualRow};
use wordcartel_core::style::LineRender;

/// One gutter cell for a ventilated paragraph's visual row (Task 6 fills these).
/// `Count(n)` is a row-group's FIRST row (the word count, `n` already clamped to `GUTTER_MAX`);
/// `Continuation` is a soft-wrap row (blank numeric field, dim `│` only).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GutterCell { Count(u16), Continuation }

/// Metadata for one ventilated PARAGRAPH window, keyed in `View.vent_blocks` by its FIRST logical
/// line. Separates the two axes the resolver needs: `last_line` for line-index LOOKUP, `byte_origin`
/// (= `ps`, the `paragraph_range_at` window start — the selector's origin) for the byte OFFSET.
/// `gutter[i]` is the cell for `line_layouts[anchor].0[i]`.
#[derive(Clone, Debug)]
pub struct VentBlock {
    pub last_line: usize,
    pub byte_origin: usize,
    pub gutter: Vec<GutterCell>,
}

/// A resolved cached layout for a logical line: the row-group's rows + ColMap, plus the byte ORIGIN
/// every consumer must reconstruct global offsets against (`origin + vr.src_span`, `head − origin`).
pub struct Resolved<'a> {
    pub rows: &'a [VisualRow],
    pub map: &'a ColMap,
    pub byte_origin: usize,
    pub first_line: usize,
    pub last_line: usize,
}

/// `(first_line, last_line)` covered by a window `[ps, pe)` — LOOKUP-range endpoints. `last_line` is
/// the line containing the window's last CONTENT byte (`pe` is exclusive; guard degenerate windows).
pub fn vent_block_range(buf: &TextBuffer, ps: usize, pe: usize) -> (usize, usize) {
    let first = buf.byte_to_line(ps.min(buf.len()));
    let last_byte = pe.saturating_sub(1).max(ps).min(buf.len().saturating_sub(1).max(0));
    (first, buf.byte_to_line(last_byte))
}

/// The shared window-aware resolver. Given any logical line `l`, return the cached entry that covers
/// it AND its byte ORIGIN — **line-index LOOKUP, `ps` OFFSET (the `paragraph_range_at` window start);
/// `line_start(l)` used for NEITHER in the ventilated path** (§5.2). `None` when no cached entry
/// covers `l` (the caller then lays the window out on-demand, Task 4).
///
/// LOOKUP: `range(..=l).next_back()` finds the candidate anchor; if it is a ventilated window
/// (`vent_blocks`), confirm `l ∈ first_line..=last_line` (a LINE-INDEX comparison, never a byte
/// comparison). Otherwise it is an ordinary per-line entry, which covers `l` only when keyed exactly
/// at `l`.
pub fn resolve<'a>(view: &'a crate::editor::View, buf: &TextBuffer, l: usize) -> Option<Resolved<'a>> {
    let (&anchor, (rows, map)) = view.line_layouts.range(..=l).next_back()?;
    if let Some(vb) = view.vent_blocks.get(&anchor) {
        if l <= vb.last_line {
            return Some(Resolved { rows, map, byte_origin: vb.byte_origin, first_line: anchor, last_line: vb.last_line });
        }
        return None; // past this block; not covered by it
    }
    if anchor == l {
        return Some(Resolved { rows, map, byte_origin: buf.line_to_byte(l), first_line: l, last_line: l });
    }
    None // an ordinary per-line entry keyed below l does not cover l
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p wordcartel resolver_resolves_interior_line` (passes now);
`cargo test -p wordcartel t_indent_origin` (passes after Task 5, or is `#[ignore]`d until then).
Expected: the per-line test PASSES; the indent test PASSES or is ignored per Step 1's note.

- [ ] **Step 6: Full gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets && cargo test -p wordcartel --test module_budgets`
Expected: green; budgets 5/5 (resolver lives in `ventilate.rs`, not a hub).

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/ventilate.rs wordcartel/src/editor.rs
git commit -m "feat(ventilate): window-aware resolver (line-index lookup + ps origin) + View.vent_blocks (S6)"
# + trailers
```

---

## Task 4 — Migrate ALL consumers (nav + render) to the resolver origin

**Files:**
- Modify: `wordcartel/src/nav.rs` (`get_or_layout`, `layout_line_on_demand`, `layout_line_active`,
  and the cross-line-transition sites)
- Modify: `wordcartel/src/render.rs` (`paint_rows` focus-dim origin; `row_spans_placed` diag/search/
  sel origin; `gather_row_ctx` search-window bounds)
- Modify: `wordcartel/src/ventilate.rs` (add the on-demand `layout_block_on_demand` + `origin_of`)

**Command-surface conformance:** N/A — internal geometry migration; no command/menu/hint change.

**Interfaces:**
- Consumes: `ventilate::resolve` (Task 3); the Task 5 fill (`ventilate::fill_visible`) is NOT yet
  wired, so under this task `vent_blocks` is empty and every consumer's behavior is **identical to
  today** — the migration is verified flag-OFF here; flag-ON correctness is covered by Task 8.
- Produces:
  - `pub fn origin_of(view: &crate::editor::View, buf: &TextBuffer, l: usize) -> usize` — the byte
    origin render must use for a cached entry keyed at `l`: `ps` (the window's `byte_origin`) if `l`
    is a ventilated window anchor, else `line_start(l)`. (Render iterates keys, so `l` is always an
    anchor key here.)
  - `pub fn layout_block_on_demand(editor: &crate::editor::Editor, l: usize) -> (ColMap, usize)` —
    the window-aware on-demand fallback for nav: returns `(map, byte_origin)` reproducing the
    ventilated window-scoped geometry for line `l` (or the ordinary per-line layout when ventilate is
    off / the line is verbatim). Mirrors `get_or_layout`'s owned-clone-or-compute contract.

- [ ] **Step 1: Write the failing test** — a flag-OFF equivalence test (this task must not change
  behavior when the lens is off) in `nav.rs` tests:

```rust
    #[test]
    fn resolver_origin_matches_line_start_when_ventilate_off() {
        // With ventilate OFF, origin_of MUST equal line_start for every cached line — the migration
        // is a no-op on the existing path.
        let mut e = Editor::new_from_text("alpha\nbeta\ngamma\n", None, (80, 24));
        crate::derive::rebuild(&mut e);
        let buf = e.active().document.buffer.clone();
        for l in 0..3usize {
            let got = crate::ventilate::origin_of(&e.active().view, &buf, l);
            assert_eq!(got, buf.line_to_byte(l), "ventilate off → origin is line_start for line {l}");
        }
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel resolver_origin_matches_line_start_when_ventilate_off`
Expected: FAIL — `cannot find function 'origin_of'`.

- [ ] **Step 3: Add `origin_of` + `layout_block_on_demand`** to `ventilate.rs`:

```rust
/// The byte origin render uses for a cached entry keyed at `l`: `ps` (the window `byte_origin`) for a
/// ventilated window anchor, else `line_start(l)`. Render iterates `line_layouts` keys, so `l` is an
/// anchor key.
pub fn origin_of(view: &crate::editor::View, buf: &TextBuffer, l: usize) -> usize {
    view.vent_blocks.get(&l).map(|vb| vb.byte_origin).unwrap_or_else(|| buf.line_to_byte(l))
}

/// Window-aware on-demand layout for nav (owned, mirrors `get_or_layout`). When `l` falls in a
/// ventilated PARAGRAPH window, reproduce that window's row-group geometry + `ps` origin; otherwise
/// fall back to the ordinary per-line layout at `line_start(l)`. Used by the nav fallbacks so an
/// interior line never reintroduces per-line geometry (the SEE==SELECT hazard, §5.2).
pub fn layout_block_on_demand(editor: &crate::editor::Editor, l: usize) -> (ColMap, usize) {
    let buf = &editor.active().document.buffer;
    // Fast path: the block is cached → clone its map + origin via the resolver.
    if let Some(r) = resolve(&editor.active().view, buf, l) {
        if editor.active().view.vent_blocks.contains_key(&r.first_line) {
            return (r.map.clone(), r.byte_origin);
        }
    }
    // Ventilated but not cached (off-screen), or verbatim/flag-off → recompute for THIS line's block.
    let blocks = editor.active().document.blocks();
    let ls = crate::derive::line_start(buf, l);
    if editor.active().view.ventilate {
        if let Some((bs, be)) = prose_block_at(blocks, buf, ls) {
            let raw = buf.slice(bs..be);
            let vp = crate::nav::text_geometry(editor).text_width as usize;
            let render = crate::derive::line_render_for(editor.active().view.mode, false); // L1
            // Rebuild the block's single combined ColMap the fill produces (Task 5 shares this).
            let (_, map, _) = layout_block(&raw, bs, vp, render, editor.theme.heading_level_glyph);
            return (map, bs);
        }
    }
    // Ordinary per-line layout (flag off or verbatim block).
    (crate::nav::layout_line_on_demand_map(editor, l), buf.line_to_byte(l))
}
```

> `layout_block` (the block→(rows, combined ColMap) builder) and `nav::layout_line_on_demand_map`
> (a thin rename exposing the existing on-demand ColMap builder) are produced in Task 5 and a
> one-line extraction respectively. If executing strictly in order, Task 4 may stub
> `layout_block_on_demand` to the ordinary per-line path (ventilate always off during Task 4 tests)
> and complete the ventilated arm in Task 5 — note this in the commit. The flag-OFF equivalence test
> (Step 1) passes either way.

- [ ] **Step 4: Migrate the render consumers** (behavior-preserving when `vent_blocks` empty):

  **`paint_rows`** (`render.rs:757-762`) — replace the `line_off` origin with the resolver origin:

```rust
            // Determine whether this visual row is dim (outside the active region).
            let row_dim = if let Some((from, to)) = ctx.focus_region {
                let buf = &editor.active().document.buffer;
                let origin = crate::ventilate::origin_of(&editor.active().view, buf, l);
                let g_from = origin + vr.src_span.start;
                let g_to = origin + vr.src_span.end;
                !row_is_active(g_from, g_to, from, to)
            } else { false };
```

  **`row_spans_placed`** (`render.rs:652, 657-658`) — same origin swap:

```rust
    let buf = &editor.active().document.buffer;
    let line_off = crate::ventilate::origin_of(&editor.active().view, buf, l);
    let mut spans: Vec<Span<'static>> = Vec::new();
    // Compute the visible byte span for this visual row so we can window the diagnostics.
    // src_span is relative to the entry ORIGIN (ps under ventilate, else line start).
    let lo = line_off + vr.src_span.start;
    let hi = line_off + vr.src_span.end;
```

  **`gather_row_ctx` search window** (`render.rs:530-533`) — bound by the last cached entry's block
  END (via the resolver), not a raw `line_start` of the anchor:

```rust
            let buf = &editor.active().document.buffer;
            let lo = derive::line_start(buf, scroll);
            // Conservative upper bound: the end of the last cached entry's coverage. Under ventilate
            // the last key is a block anchor, so bound by that block's last line + 1 (resolver),
            // never a raw line_start of the anchor (which would clip matches inside the block).
            let max_visible = sorted_lines.last().copied().unwrap_or(scroll);
            let end_line = crate::ventilate::resolve(&editor.active().view, buf, max_visible)
                .map(|r| r.last_line).unwrap_or(max_visible);
            let hi = derive::line_start(buf, end_line + 1);
```

- [ ] **Step 5: Migrate the nav consumers** — route `get_or_layout`, `layout_line_on_demand`, and
  `layout_line_active` through the window-aware path. The minimal change: each already returns an
  owned `ColMap`; make the callers that combine it with `line_start` take the origin from the
  resolver. Concretely, `screen_pos` (`nav.rs:107-108`), `clamp_snap` (`nav.rs:169-171`), the
  `move_*` line-transition sites (`nav.rs:198, 230, 329, 382`), `rows_before_caret` (`nav.rs:532`),
  and the click map (`nav.rs:983-984`) replace `let line_off = line_start(buf, l)` /
  `h - line_start(l)` with the resolver origin. Example — `screen_pos` (`nav.rs:107-111`):

```rust
    let (map, origin) = crate::ventilate::layout_block_on_demand(editor, l);
    let in_off = h.saturating_sub(origin);
    // Snap to a valid cursor stop before calling source_to_visual
    let snapped = map.snap_to_stop(in_off);
    let (vrow, vcol) = map.source_to_visual(snapped);
```

  Apply the same `(map, origin) = layout_block_on_demand(editor, l)` pattern at each site listed
  above (replacing the local `get_or_layout(editor, l)` + `line_start` pair). `layout_line_active`
  (`nav.rs:142`) — the cross-line-transition helper — is superseded by `layout_block_on_demand` at
  its call sites (`nav.rs:198, 230, 329, 382`); replace each `layout_line_active(editor, l+1)` +
  `line_start(l+1)` pair with `layout_block_on_demand(editor, l+1)`.

  > **Grounding note for the implementer:** each nav site currently pairs an owned ColMap with a
  > `line_start`-derived offset. The mechanical rule: wherever the code computes
  > `in_off = head - line_start(l)` OR `next_ls = line_start(l+1)` for a cached/on-demand map, take
  > BOTH the map and the origin from `layout_block_on_demand(editor, l)`. Do NOT leave any
  > `line_start(l)` feeding a ColMap offset in these fns. Verify with
  > `grep -n "line_start" wordcartel/src/nav.rs` that every remaining `line_start` use is for logical
  > line arithmetic (line text, byte ranges), not a ColMap origin.

- [ ] **Step 6: Run tests**

Run: `cargo test -p wordcartel resolver_origin_matches_line_start_when_ventilate_off`
Then the existing nav and render suites separately (each must stay green — flag OFF is unchanged
behavior): `cargo test -p wordcartel nav::` then `cargo test -p wordcartel render::`
Expected: PASS; no regression (all existing motion/click/focus/search render tests green).

- [ ] **Step 7: Full gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets && cargo test -p wordcartel --test module_budgets`
Expected: green; `render.rs` under 900 (the origin swaps are one-liners; the search-window change is
~3 lines).

- [ ] **Step 8: Commit**

```bash
git add wordcartel/src/nav.rs wordcartel/src/render.rs wordcartel/src/ventilate.rs
git commit -m "refactor(nav,render): take global-offset origin from the window-aware resolver (S6)"
# + trailers
```

---

## Task 5 — Lens layout path in `derive::rebuild` (thin branch) + `layout()` `reserve_cols`

**Files:**
- Modify: `wordcartel-core/src/layout.rs` (add `reserve_cols` param to `layout`)
- Modify: `wordcartel/src/ventilate.rs` (add `layout_block` + `fill_visible`)
- Modify: `wordcartel/src/derive.rs` (thin `if view.ventilate` branch in `rebuild_downstream`)
- Modify: `wordcartel/src/nav.rs` (pass `0`/reduced `reserve_cols` at existing `layout::layout` calls;
  add `layout_line_on_demand_map`)

**Command-surface conformance:** N/A — layout mechanics.

**Interfaces:**
- Consumes: `layout::layout` (new signature); `ventilate::{prose_block_at, sentence_display,
  segment_block, vent_block_range, GUTTER_COLS, VentBlock}`.
- Produces:
  - `pub fn layout(line: &str, role: BlockRole, render: LineRender, viewport_width: usize, heading_prefix: bool, reserve_cols: usize) -> (Vec<VisualRow>, ColMap)`
    — `reserve_cols` is added to the row's `prefix_width` (offsets every placed col; continuation
    rows hang-indent to it), so a ventilated sentence laid out with `reserve_cols = GUTTER_COLS`
    reserves the 6 gutter columns and ALL cursor math (via `ColMap.prefix_width`) is correct by
    construction. `0` = today's behavior exactly.
  - `pub fn layout_block(raw: &str, ps: usize, vp_width: usize, render: LineRender, heading_glyph: bool) -> (Vec<VisualRow>, ColMap, Vec<GutterCell>)`
    (in `ventilate.rs`) — `raw` = `buf.slice(ps..pe)`; segment RAW (offsets window-relative to `ps`,
    i.e. into `raw`), lay out each sentence's DISPLAY string at `vp_width` with `reserve_cols =
    GUTTER_COLS`, and STITCH the row-groups into one combined `(rows, ColMap)` whose `src` offsets stay
    **window-relative to `ps`** (global = `ps + src`, added by the resolver's `byte_origin`), plus the
    per-row `Vec<GutterCell>`. `render` is `line_render_for(view.mode, false)` — L1: a ventilated prose
    row is NEVER the "active" raw line, so under LivePreview it is `Concealed` (clean), under Source
    modes `RawPlain`/`RawStyled` (raw markers on every sentence row, §6.1). Returns the per-window
    entry + gutter the fill caches.
  - `pub fn fill_visible(editor: &mut Editor)` (in `ventilate.rs`) — the window-scoped replacement for
    `rebuild_downstream`'s per-line loop: walk fold-visible lines, classify, gather+lay out prose
    blocks (caching an anchor entry + `VentBlock`), lay out verbatim blocks per-line with
    `reserve_cols = GUTTER_COLS` (reserved-blank gutter, §5.4). Populates `view.line_layouts` +
    `view.vent_blocks`.
  - `pub fn layout_line_on_demand_map(editor: &Editor, l: usize) -> ColMap` (in `nav.rs`) — a rename
    exposing the existing `layout_line_on_demand` body for `ventilate::layout_block_on_demand`.

- [ ] **Step 1: Write the failing test** — in `ventilate.rs` tests, assert the block path produces
  ONE row-group per sentence and the combined ColMap round-trips a former-newline byte:

```rust
    #[test]
    fn fill_produces_one_rowgroup_per_sentence_and_reflows_hard_wrap() {
        // A paragraph whose first sentence hard-wraps across two logical lines.
        let text = "The committee met on Tuesday and the\nchair insisted on a vote. Then we left.\n";
        let mut e = Editor::new_from_text(text, None, (30, 24));
        e.active_mut().view.ventilate = true;
        crate::derive::rebuild(&mut e);
        // The block is anchored at line 0 with a VentBlock; sentence 1 spans the hard newline.
        let vb = e.active().view.vent_blocks.get(&0).expect("paragraph anchored at line 0");
        assert!(vb.last_line >= 1, "block covers the hard-wrapped second logical line");
        // Combined ColMap: the byte at the former '\n' (index of '\n' in the source) maps and
        // round-trips (it became a space in DISPLAY but is a real buffer byte).
        let (rows, map) = &e.active().view.line_layouts[&0];
        let nl = text.find('\n').unwrap(); // global byte of the hard newline (block_start == 0 here)
        let (r, c) = map.source_to_visual(nl);
        assert_eq!(map.visual_to_source(r, c), map.snap_to_stop(nl), "former-newline byte round-trips");
        assert!(!rows.is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel fill_produces_one_rowgroup_per_sentence`
Expected: FAIL — `fill_visible`/`vent_blocks` not populated (empty map → panic on `[&0]`).

- [ ] **Step 3: Add `reserve_cols` to core `layout()`.** In `layout.rs`, change the signature
  (`layout.rs:244-250`) and fold `reserve_cols` into `prefix_width`:

```rust
pub fn layout(
    line: &str,
    role: BlockRole,
    render: LineRender,
    viewport_width: usize,
    heading_prefix: bool,
    reserve_cols: usize,
) -> (Vec<VisualRow>, ColMap) {
```

  After the existing `prefix_width` computation (`layout.rs:284-297`, the block-glyph +
  heading-placeholder logic), add the reservation before the wrap loop initializes `col`:

```rust
    // S6: the ventilate gutter is a left column reservation — add it to prefix_width so every
    // placed col offsets by it and continuation rows hang-indent to it (cursor math stays correct
    // via ColMap.prefix_width). 0 for every non-ventilated caller (behavior identical).
    prefix_width += reserve_cols;
```

  (`prefix_width` is already `let mut` at `layout.rs:284`. The `col = prefix_width` init at
  `layout.rs:308` and the `ColMap { … prefix_width … }` construction then carry the reservation
  automatically.)

  Update EVERY existing `layout::layout(...)` call to pass `0` as the new last arg. Grounded call
  sites: `derive.rs:267`, `nav.rs:66` (`layout_line_on_demand`), `nav.rs:147` (`layout_line_active`),
  and any layout tests in `layout.rs`/`derive.rs`/`render.rs`. Find them all:
  `grep -rn "layout::layout(\|layout(&text\|layout(line" wordcartel wordcartel-core`.

- [ ] **Step 4: Add `layout_block` + `fill_visible`** to `ventilate.rs`:

```rust
/// Lay out one PROSE window `raw` (= `buf.slice(ps..pe)`): segment RAW (offsets window-relative to
/// `ps`), lay out each sentence's DISPLAY string at `vp_width` with the 6-col gutter reserved, and
/// stitch the row-groups into ONE combined `(rows, ColMap)` whose `src` offsets stay WINDOW-relative
/// (global = `ps + src`, added by the resolver's `byte_origin`). `ps` is passed for documentation
/// only — the ColMap is NOT globally offset here. The `gutter` cells (Count on each group's first
/// row, Continuation on wraps) travel to the caller.
pub fn layout_block(raw: &str, ps: usize, vp_width: usize, render: LineRender, heading_glyph: bool)
    -> (Vec<VisualRow>, ColMap, Vec<GutterCell>) {
    use wordcartel_core::layout::{self, Placed};
    use wordcartel_core::style::BlockRole;
    let mut rows: Vec<VisualRow> = Vec::new();
    let mut placed: Vec<Placed> = Vec::new();
    let mut row_end_col: Vec<usize> = Vec::new();
    let mut gutter: Vec<GutterCell> = Vec::new();
    let mut row_base = 0usize; // running visual-row offset across row-groups
    for (sf, st) in segment_block(raw) {
        let words = wordcartel_core::count::word_count(&raw[sf..st]).min(GUTTER_MAX as usize) as u16;
        let display = sentence_display(&raw[sf..st]); // \n → space, byte-length-preserving
        // Paragraph prose, `render` per view.mode (L1: never the active raw line; §6.1).
        let (mut srows, smap) =
            layout::layout(&display, BlockRole::Paragraph, render, vp_width, heading_glyph, GUTTER_COLS);
        for (i, vr) in srows.iter_mut().enumerate() {
            // Shift this sentence's src spans from sentence-relative → window-relative (to ps).
            vr.src_span = (vr.src_span.start + sf)..(vr.src_span.end + sf);
            gutter.push(if i == 0 { GutterCell::Count(words) } else { GutterCell::Continuation });
        }
        // Stitch the sentence ColMap's placed cells (row-shift + src-shift) into the window ColMap.
        for p in &smap.placed {
            placed.push(Placed {
                src: (p.src.start + sf)..(p.src.end + sf),
                row: p.row + row_base, col: p.col, width: p.width,
                text: p.text.clone(), style: p.style,
            });
        }
        for rec in &smap.row_end_col { row_end_col.push(*rec); }
        row_base += smap.rows;
        rows.append(&mut srows);
    }
    let eol = raw.len();
    let map = ColMap {
        placed, rows: row_base.max(1), eol,
        row_end_col, is_active: false, prefix_width: GUTTER_COLS,
    };
    let _ = ps; // src offsets are window-relative; the resolver adds ps as byte_origin
    (rows, map, gutter)
}

/// The ventilate replacement for `rebuild_downstream`'s per-line fill: walk fold-visible logical
/// lines from `first_line`, classify each block, cache a PROSE block as one anchor entry + a
/// `VentBlock`, a VERBATIM block per-line (reserved-blank gutter). Off-screen blocks are never
/// gathered (§4.3, L3). Populates `view.line_layouts` + `view.vent_blocks`.
pub fn fill_visible(editor: &mut Editor); // full body in Step 5 below (declared here for the interface)
```

  > **Grounding — `ColMap` construction:** `ColMap { placed, rows, eol, row_end_col, is_active,
  > prefix_width }` (`layout.rs:65-82`) — all six fields are `pub`. `Placed { src, row, col, width,
  > text, style }` (`layout.rs:13-26`) — all `pub`. So `ventilate.rs` (shell) can construct both. The
  > stitched `row_end_col` preserves per-row end columns for `col_on_row`/`source_to_visual`.

- [ ] **Step 5: Implement `fill_visible`** and the thin `derive` branch. `fill_visible` mirrors the
  existing loop (`derive.rs:257-273`) but window-aware:

```rust
pub fn fill_visible(editor: &mut Editor) {
    use wordcartel_core::style::{BlockRole, LineRender};
    let fold_view = editor.active_fold_view();
    let total = crate::derive::total_logical_lines(&editor.active().document.buffer);
    let (area_height, first_line, scroll_row) = {
        let v = &editor.active().view;
        (v.area.1 as usize, v.scroll, v.scroll_row)
    };
    let vp = crate::nav::text_geometry(editor).text_width as usize;
    let heading_glyph = editor.theme.heading_level_glyph;
    let mode = editor.active().view.mode;
    // L1 — a ventilated PROSE row is NEVER the active raw line; `is_active = false` gives Concealed
    // under LivePreview and raw markers under Source modes (§6.1). Verbatim rows keep the REAL
    // is_active (IMPORTANT 3 / §4.2): an active heading still reveals its raw markup.
    let prose_render = crate::derive::line_render_for(mode, false);
    let active_line = {
        let b = editor.active();
        let caret = b.document.selection.primary().head;
        if b.document.buffer.is_empty() { 0 } else { b.document.buffer.byte_to_line(caret.min(b.document.buffer.len())) }
    };
    editor.active_mut().view.line_layouts.clear();
    editor.active_mut().view.vent_blocks.clear();
    #[cfg(test)]
    crate::derive::LAYOUT_RUNS.with(|c| c.set(c.get() + 1));
    let overscan = area_height.saturating_add(scroll_row).saturating_add(1);
    let mut acc = 0usize;
    let mut l = first_line;
    while l < total && acc < overscan {
        let ls = crate::derive::line_start(&editor.active().document.buffer, l);
        let prose = {
            let b = editor.active();
            crate::ventilate::prose_block_at(b.document.blocks(), &b.document.buffer, ls)
        };
        if let Some((ps, pe)) = prose {
            let raw = editor.active().document.buffer.slice(ps..pe);
            let (rows, map, gutter) = crate::ventilate::layout_block(&raw, ps, vp, prose_render, heading_glyph);
            let (first, last) = crate::ventilate::vent_block_range(&editor.active().document.buffer, ps, pe);
            acc += rows.len();
            // The ColMap's `src`/`src_span` stay WINDOW-RELATIVE (to `ps`); the resolver returns
            // `byte_origin = ps`, and consumers reconstruct globals as `byte_origin + src`. So the
            // entry is inserted as-is — no offset rewrite (§5.2).
            editor.active_mut().view.line_layouts.insert(first, (rows, map));
            editor.active_mut().view.vent_blocks.insert(first, crate::ventilate::VentBlock {
                last_line: last, byte_origin: ps, gutter,
            });
            l = fold_view.next_visible(last).unwrap_or(total);
        } else {
            // Verbatim block: existing per-line layout with the REAL is_active (IMPORTANT 3), gutter
            // column reserved BLANK for GLYPHLESS rows (§5.4). Glyph-carrying rows (list/blockquote
            // bullet/bar) keep today's geometry (reserve 0) to avoid glyph/cursor desync — a minor
            // left-inset accepted as deferred-verbatim residue (§5.4).
            let (text, role) = {
                let b = editor.active();
                (crate::derive::line_text(&b.document.buffer, l), b.document.blocks().role_at(ls))
            };
            let render = crate::derive::line_render_for(mode, l == active_line);
            // Lay out at reserve 0 first; if the row carries NO prefix glyph, re-lay reserving the
            // 6-col blank gutter so verbatim text aligns with prose (cold path, once).
            let (rows0, map0) = wordcartel_core::layout::layout(&text, role, render, vp, heading_glyph, 0);
            let (rows, mapl) = if rows0.first().map_or(true, |r| r.prefix_glyph.is_none()) {
                wordcartel_core::layout::layout(&text, role, render, vp, heading_glyph, crate::ventilate::GUTTER_COLS)
            } else {
                (rows0, map0)
            };
            acc += rows.len();
            editor.active_mut().view.line_layouts.insert(l, (rows, mapl));
            l = fold_view.next_visible(l).unwrap_or(total);
        }
    }
}
```

  > **Consistency pin:** a ventilated prose entry's ColMap `src`/`src_span` are WINDOW-RELATIVE (to
  > `ps`), matching §5.2. `byte_origin = ps` is what the resolver returns; consumers reconstruct
  > globals as `byte_origin + src`. Verbatim per-line entries stay LINE-relative with `byte_origin =
  > line_start`. This is the single semantic every Task 4 consumer already respects.

  Then the **thin `derive` branch**. In `rebuild_downstream` (`derive.rs`), after the `LayoutKey` gate
  (`derive.rs:243-245` `if editor.active().layout_key.as_ref() == Some(&key) { return; }`), replace
  the per-line fill loop (`derive.rs:252-273`) with:

```rust
    if editor.active().view.ventilate {
        crate::ventilate::fill_visible(editor);
        editor.active_mut().layout_key = Some(key);
        return;
    }
    // IMPORTANT 2 — the non-ventilate path only clears line_layouts (derive.rs:253); it must ALSO
    // clear vent_blocks, or stale resolver metadata survives a toggle-off. (Runs on the gate miss the
    // ventilate flip causes.)
    editor.active_mut().view.vent_blocks.clear();
    // … existing per-line loop unchanged (the else path: line_layouts.clear() + the fill loop) …
```

  (Keep the existing loop verbatim as the non-ventilate path; the branch is a THIN delegation —
  `derive.rs` gains ~7 lines, stays a dispatcher, no hub growth.)

- [ ] **Step 6: Expose `layout_line_on_demand_map`** — in `nav.rs`, rename the body of
  `layout_line_on_demand` (`nav.rs:60-68`) to `pub(crate) fn layout_line_on_demand_map` (same body,
  passing `0` for `reserve_cols` when ventilate off / the resolver handles ventilated blocks). Keep a
  thin `layout_line_on_demand` wrapper if any caller outside Task 4 still uses it (grep first).

- [ ] **Step 7: Run tests**

Run: `cargo test -p wordcartel fill_produces_one_rowgroup_per_sentence`
Then un-ignore Task 3's `t_indent_origin_*` and run it:
`cargo test -p wordcartel t_indent_origin`
Expected: both PASS.

- [ ] **Step 8: Full gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets && cargo test -p wordcartel --test module_budgets`
Expected: green; `layout()` new arg propagated everywhere (no call-site left unfixed); budgets 5/5.

- [ ] **Step 9: Commit**

```bash
git add wordcartel-core/src/layout.rs wordcartel/src/ventilate.rs wordcartel/src/derive.rs wordcartel/src/nav.rs
git commit -m "feat(ventilate): window-scoped layout path + layout() reserve_cols; thin derive branch (S6)"
# + trailers
```

---

## Task 6 — The 6-col rhythm gutter render

**Files:**
- Modify: `wordcartel/src/render.rs` (`paint_rows` — paint the gutter cells)
- Modify: `wordcartel/src/ventilate.rs` (add `render_gutter_span` helper — keeps render thin/budget)

**Command-surface conformance:** N/A — paint only.

**Interfaces:**
- Consumes: `View.vent_blocks[anchor].gutter` (Task 5); `GutterCell`; `View.ventilate`.
- Produces: `pub fn gutter_span(cell: Option<GutterCell>, editor: &crate::editor::Editor) -> Vec<Span<'static>>`
  (in `ventilate.rs`) — the 6-col gutter Line prefix for one visual row: `"NNN │ "` (right-aligned
  3-digit count, dim `│`) for `Some(Count(n))`, `"    │ "` for `Some(Continuation)`, six blanks for
  `None` (verbatim rows under ventilate). Takes `&Editor` and reads `&editor.theme` / `editor.depth`
  (render's own convention, `render.rs:178`), sidestepping theme/depth type paths. Keeps the
  format/glyph logic OUT of the `render.rs` hub.

- [ ] **Step 1: Write the failing test** — an e2e gutter test in `e2e.rs`:

```rust
#[test]
fn e2e_gutter_shows_right_aligned_word_counts() {
    // Two sentences: 4 words and 2 words. Gutter shows right-aligned counts + a rule.
    let mut h = Harness::new("Alpha beta gamma delta. Bye now.\n", None, (40, 8));
    { let mut ed = h.editor.borrow_mut(); ed.active_mut().view.ventilate = true;
      crate::derive::rebuild(&mut ed); }
    h.render();
    // Row 0 carries sentence 1's count (4) right-aligned in 3 cols, then the rule.
    let r0 = h.row(0);
    assert!(r0.starts_with("  4 │ "), "row 0 gutter: `  4 │ ` — got {r0:?}");
    assert!(r0.contains("Alpha beta gamma delta"), "sentence 1 text follows the gutter");
    // Sentence 2 is on its own row-group with count 2.
    let s2 = (0..8u16).map(|y| h.row(y)).find(|r| r.contains("Bye now")).expect("sentence 2 visible");
    assert!(s2.starts_with("  2 │ "), "sentence 2 gutter: `  2 │ ` — got {s2:?}");
}

#[test]
fn e2e_gutter_clamps_to_999() {
    // A 1000-word paragraph clamps the display to 999 (§7/L7).
    let big = std::iter::repeat("word").take(1000).collect::<Vec<_>>().join(" ") + ".\n";
    let mut h = Harness::new(&big, None, (40, 8));
    { let mut ed = h.editor.borrow_mut(); ed.active_mut().view.ventilate = true;
      crate::derive::rebuild(&mut ed); }
    h.render();
    assert!(h.row(0).starts_with("999 │ "), "≥1000-word sentence clamps to 999 — got {:?}", h.row(0));
}

#[test]
fn e2e_gutter_overwrites_lead_in_no_content_shift() {
    // IMPORTANT 1: the gutter OVERWRITES the reserved lead-in — reserved width == painted width, so
    // content is NOT shifted right. The sentence text must begin at exactly column GUTTER_COLS (6),
    // and the same text under a NON-ventilated render must begin at column 0 — the only difference
    // being the 6-col gutter, never a double-count (which would push text to column 12).
    let text = "Alpha beta gamma.\n";
    let mut h = Harness::new(text, None, (40, 8));
    { let mut ed = h.editor.borrow_mut(); ed.active_mut().view.ventilate = true;
      crate::derive::rebuild(&mut ed); }
    h.render();
    let r0 = h.row(0);
    // Columns 0..6 are the gutter ("  3 │ "); the sentence text starts at column 6, not 12.
    assert_eq!(&r0[..6], "  3 │ ", "gutter occupies exactly 6 columns");
    assert!(r0[6..].starts_with("Alpha"), "content begins at col 6 (no double-count shift) — got {r0:?}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p wordcartel e2e_gutter_shows_right_aligned`
Then: `cargo test -p wordcartel e2e_gutter_clamps_to_999`
Then: `cargo test -p wordcartel e2e_gutter_overwrites_lead_in`
Expected: FAIL — no gutter painted (text starts at col 0, not `"  4 │ "`).

- [ ] **Step 3: Add the gutter span builder** to `ventilate.rs`:

```rust
use ratatui::text::Span;
use ratatui::style::Modifier;
use crate::compose;                                  // shared composer (crate::compose, lib.rs:54)
use wordcartel_core::theme::SemanticElement as SE;   // theme.rs:85 (FoldMarker @ 129, ChromeMuted @ 139)

/// The 6-column gutter prefix for one visual row (`GUTTER_COLS` wide). `Some(Count(n))` → the
/// row-group's first row: `NNN` right-aligned in 3 cols (subdued) + ` │ ` (the rule dim).
/// `Some(Continuation)` → a soft-wrap row: 3 blanks + ` │ ` (blank numeric field, rule kept).
/// `None` → a verbatim row under ventilate: 6 blanks (reserved, no rule; §5.4).
pub fn gutter_span(cell: Option<GutterCell>, editor: &crate::editor::Editor) -> Vec<Span<'static>> {
    let (theme, depth) = (&editor.theme, editor.depth);
    let dim = compose::compose(theme, depth, &[SE::FoldMarker]).add_modifier(Modifier::DIM);
    match cell {
        Some(GutterCell::Count(n)) => vec![
            Span::styled(format!("{n:>3}"), compose::compose(theme, depth, &[SE::ChromeMuted])),
            Span::styled(" │ ".to_string(), dim),
        ],
        Some(GutterCell::Continuation) => vec![Span::styled("    │ ".to_string(), dim)],
        None => vec![Span::raw("      ".to_string())],
    }
}
```

  > **Grounding (verified 2026-07-13):** `SemanticElement` is `wordcartel_core::theme::SemanticElement`
  > (theme.rs:85) — variants include `FoldMarker` (129), `ChromeMuted` (139), `Chrome` (133); there is
  > **no** `LineNumber` variant, so the subdued count uses `ChromeMuted`. `compose::compose(theme,
  > depth, &[SE::…])` is the shared composer at `wordcartel/src/compose.rs:43`, exposed as the
  > top-level `crate::compose` module (`lib.rs:54`; render imports it `use crate::{compose, …}`,
  > `render.rs:4`). Taking `&Editor` and reading `&editor.theme`/`editor.depth` matches render's own
  > call convention (`render.rs:178`) and avoids threading the `Theme`/`Depth` type paths.

- [ ] **Step 4: Paint the gutter in `paint_rows`.** In `paint_rows` (`render.rs:752-788`), inside the
  per-visual-row loop, **OVERWRITE the reserved lead-in** (NOT prepend — a prepend double-counts;
  IMPORTANT 1). A ventilated PROSE row was laid out with `prefix_width = GUTTER_COLS` and no prefix
  glyph, so `push_prefix_lead_in` (`render.rs:627-628`) already pushed exactly ONE
  `Span::raw(" ".repeat(6))` at `spans[0]`. Splice the gutter cells over that single lead-in span, so
  reserved width (6) == painted width (6), no content shift. After `let mut spans = …`
  (`render.rs:770-774`) and before the fold-marker insert:

```rust
            // Ventilated PROSE rows carry a VentBlock gutter (count/│); OVERWRITE the reserved
            // 6-space lead-in that push_prefix_lead_in emitted for the glyphless prose row. Verbatim
            // rows have NO vent_blocks entry: their reserved lead-in already reads as a blank gutter
            // (§5.4), so nothing to do; glyph-carrying verbatim rows kept reserve 0 (Task 5) and are
            // untouched. gutter_span returns exactly GUTTER_COLS columns, so width is preserved.
            if let Some(cell) = editor.active().view.vent_blocks.get(&l)
                .and_then(|vb| vb.gutter.get(row_index).copied())
            {
                let gutter = crate::ventilate::gutter_span(Some(cell), editor);
                if spans.is_empty() { spans = gutter; } else { spans.splice(0..1, gutter); }
            }
```

  > **Grounding (verified 2026-07-13):** `push_prefix_lead_in`'s no-glyph branch (`render.rs:627-628`)
  > is `else if map.prefix_width > 0 { spans.push(Span::raw(" ".repeat(map.prefix_width))); }` — for a
  > glyphless prose row (`prefix_width == 6`) that is exactly one 6-wide span at index 0, so
  > `splice(0..1, gutter)` replaces it and total width is unchanged (`gutter_span` sums to 6). This is
  > the OVERWRITE the finding requires — never an `append`/prepend. The caret (screen x =
  > `text_left + col`, `col ≥ prefix_width = 6`) lands correctly because the ColMap already offsets by
  > 6. Step 5 adds a width-equality assertion so a future prepend/double-count regresses loudly.

- [ ] **Step 5: Run tests**

Run each filter separately: `cargo test -p wordcartel e2e_gutter_shows_right_aligned`, then
`cargo test -p wordcartel e2e_gutter_clamps_to_999`, then
`cargo test -p wordcartel e2e_gutter_overwrites_lead_in`.
Expected: PASS.

- [ ] **Step 6: Full gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets && cargo test -p wordcartel --test module_budgets`
Expected: green; **`render.rs` ≤ 900** (the paint addition is ~8 lines; `gutter_span` lives in
`ventilate.rs`).

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/render.rs wordcartel/src/ventilate.rs
git commit -m "feat(ventilate): 6-col rhythm gutter render — word count, 999 clamp, continuation rule (S6)"
# + trailers
```

---

## Task 7 — Command surface: `toggle_ventilate` + shared setter + `alt-v`

**Files:**
- Modify: `wordcartel/src/ventilate.rs` (the shared setter `set_ventilate`)
- Modify: `wordcartel/src/registry.rs` (`register_stateful` `toggle_ventilate` row)
- Modify: `wordcartel/src/keymap.rs` (CUA `alt-v` row)

**Command-surface conformance (per the contract):** **Law 2/10** — `toggle_ventilate` is the command
for the `View.ventilate` option, nullary. **Shape rule 8** — a boolean option ⇒ a single `MenuMark::
OnOff` toggle representative (the `toggle_measure` shape), no set-per-state/cycle. **Law 6** — ONE
shared setter (`ventilate::set_ventilate`) flips the flag + `derive::rebuild`; the registry closure
calls it (a Law-6-required improvement over `toggle_measure`'s inline closure — §8/L5). **Law 3** —
the row appears in the palette automatically (gated by `palette_is_exhaustive_over_the_registry`).
**Law 4** — the menu row names a registered command → in the palette ✓. **Law 7** — `alt-v` hints in
CUA, none in WordStar; re-resolution gated by `hints_reresolve_on_preset_switch` +
`custom_bind_surfaces_in_menu_and_palette`. **Law 2 guard** — N/A: `View.ventilate` is per-buffer
session state, not a `SettingsSnapshot` field. **No `Command` enum variant** (L5). Row + keymap land
in ONE commit so no gate sees a half-state.

**Interfaces:**
- Consumes: `register_stateful` (`registry.rs:118`); `MenuMark::OnOff` (`registry.rs:82`);
  `MenuCategory::View`; `derive::rebuild`.
- Produces: `pub fn set_ventilate(editor: &mut Editor, on: bool)` (in `ventilate.rs`) — the single
  setter: sets `active_mut().view.ventilate = on` then `derive::rebuild`. Registry id
  `"toggle_ventilate"`, CUA `alt-v`.

- [ ] **Step 1: Write the failing tests** — registry (stateful + effect) and keymap (chord):

  In `registry.rs` `#[cfg(test)]`:

```rust
    #[test]
    fn toggle_ventilate_is_stateful_onoff_and_flips_the_flag() {
        let reg = Registry::builtins();
        let mut ed = crate::editor::Editor::new_from_text("Hi there. Bye.\n", None, (40, 8));
        let m = reg.meta(CommandId("toggle_ventilate")).unwrap();
        assert_eq!(m.menu, Some(MenuCategory::View), "toggle_ventilate is a View row");
        let f = m.state.expect("toggle_ventilate is stateful");
        assert!(matches!(f(&ed), MenuMark::OnOff(false)), "defaults off");
        // Dispatch flips it on and rebuilds.
        let ex = InlineExecutor::default();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut ctx = Ctx { editor: &mut ed, clock: &Z, executor: &ex, msg_tx: tx };
        assert_eq!(reg.dispatch(CommandId("toggle_ventilate"), &mut ctx), CommandResult::Handled);
        assert!(ed.active().view.ventilate, "dispatch turned the lens on");
        assert!(matches!(f(&ed), MenuMark::OnOff(true)));
    }
```

  In `keymap.rs` `#[cfg(test)]`:

```rust
    #[test]
    fn ventilate_bound_alt_v_in_cua_unbound_in_wordstar() {
        let reg = Registry::builtins();
        let seq = |s: &str| parse_seq(s).unwrap();
        let (cua, warns) = build_keymap(
            &crate::config::KeymapConfig { preset: "cua".into(), patches: vec![] }, &reg);
        assert!(warns.is_empty(), "cua warns: {warns:?}");
        assert!(matches!(cua.resolve(&seq("alt-v")), Resolution::Command(CommandId("toggle_ventilate"))));
        let (ws, warns) = build_keymap(
            &crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] }, &reg);
        assert!(warns.is_empty(), "wordstar warns: {warns:?}");
        assert!(!matches!(ws.resolve(&seq("alt-v")), Resolution::Command(CommandId("toggle_ventilate"))),
            "WordStar deliberately unbound (law 7: palette-only, no hint)");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p wordcartel toggle_ventilate_is_stateful`, then
`cargo test -p wordcartel ventilate_bound_alt_v`
Expected: FAIL — `toggle_ventilate` not registered.

- [ ] **Step 3: Add the shared setter** to `ventilate.rs`:

```rust
/// The single setter for the per-buffer ventilate lens (command-surface Law 6 — profiles/plugins
/// call THIS, never a bypass). Flips the flag on the ACTIVE buffer and rebuilds so the layout path
/// switches. `derive::rebuild` is required on flip (the `measure` precedent) — the LayoutKey change
/// alone would re-run, but rebuilding here keeps the setter self-contained for non-command callers.
pub fn set_ventilate(editor: &mut crate::editor::Editor, on: bool) {
    editor.active_mut().view.ventilate = on;
    crate::derive::rebuild(editor);
}
```

- [ ] **Step 4: Register the command.** In `registry.rs`, after the `toggle_measure` block
  (`registry.rs:553-555`), mirroring its `register_stateful` shape:

```rust
        r.register_stateful("toggle_ventilate", "Toggle Ventilate View", Some(MenuCategory::View),
            |e| MenuMark::OnOff(e.active().view.ventilate),
            |c| { crate::ventilate::set_ventilate(c.editor, !c.editor.active().view.ventilate); CommandResult::Handled });
```

- [ ] **Step 5: Bind `alt-v` in CUA.** In `keymap.rs` `static CUA` (`keymap.rs:257`), among the
  alt-plane rows (near the view toggles):

```rust
    // Ventilate lens (S6) — non-destructive sentence-per-line view.
    ("alt-v",       "toggle_ventilate"),
```

  **WORDSTAR** — no row added (deliberately unbound; palette-only, no hint).

- [ ] **Step 6: Run tests + contract gates**

Run each filter separately: `cargo test -p wordcartel toggle_ventilate_is_stateful`,
`cargo test -p wordcartel ventilate_bound_alt_v`, `cargo test -p wordcartel palette_is_exhaustive`,
`cargo test -p wordcartel hints_reresolve`, `cargo test -p wordcartel custom_bind_surfaces`
Expected: PASS — the new row is in the palette; hints re-resolve; `alt-v` resolves in CUA only.

- [ ] **Step 7: Full gates**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets`
Expected: green.

- [ ] **Step 8: Commit**

```bash
git add wordcartel/src/ventilate.rs wordcartel/src/registry.rs wordcartel/src/keymap.rs
git commit -m "feat(commands): toggle_ventilate (View, OnOff) + shared setter + CUA alt-v (S6)"
# + trailers
```

---

## Task 8 — Integration/e2e + guardrails

**Files:**
- Modify: `wordcartel/src/e2e.rs` (SEE==SELECT, idempotence, paint-origin, focus-window, compose)
- Modify: `wordcartel/src/ventilate.rs` (`t-diff-spans`, perf guardrail — unit-level)

**Command-surface conformance:** N/A — test-only.

**Interfaces:**
- Consumes: everything from Tasks 1–7; `Harness` (`e2e.rs`), `dim_cols`/`underlined_cols`/`row`,
  `crate::derive::LAYOUT_RUNS`, `crate::commands::run` (for `select_sentence`).
- Produces: the spec §12 guardrail suite.

- [ ] **Step 1: SEE==SELECT + idempotence + diff-spans** — in `e2e.rs` / `ventilate.rs`:

```rust
// ventilate.rs tests:
    #[test]
    fn t_diff_spans_equals_sentence_spans_on_raw() {
        // The lens's row-group grouping equals sentence_spans on the RAW block text — incl. a
        // verse (two-space) hard break that the veto keeps as TWO groups (matching select-sentence).
        let raw = "Roses are red,  \nViolets are blue. Then dawn.";
        let want = wordcartel_core::textobj::sentence_spans(raw).count();
        assert_eq!(segment_block(raw).count(), want);
        assert_eq!(want, 3, "verse hard break + terminator → three sentences");
    }

    #[test]
    fn t_perf_edit_relays_visible_range_only() {
        // Editing in a ventilated block re-runs the fill for the VISIBLE range only, never an
        // O(document) walk. LAYOUT_RUNS increments by exactly 1 for the post-edit rebuild.
        let mut e = Editor::new_from_text("Alpha one. Beta two.\n\nGamma three.\n", None, (40, 8));
        e.active_mut().view.ventilate = true;
        crate::derive::rebuild(&mut e);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5);
        let (cs, edit) = crate::commands::build_multi_replace(&[(5, 5, "X".into())], e.active().document.buffer.len());
        let txn = wordcartel_core::history::Transaction::new(cs);
        struct C; impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { 0 } }
        e.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C);
        crate::derive::LAYOUT_RUNS.with(|c| c.set(0));
        crate::derive::rebuild(&mut e);
        assert_eq!(crate::derive::LAYOUT_RUNS.with(|c| c.get()), 1, "one visible-range fill, no doc walk");
    }
```

```rust
// e2e.rs tests:
#[test]
fn e2e_see_equals_select_hard_wrapped_sentence() {
    // A sentence hard-wrapped across two logical lines is ONE row-group; select_sentence with the
    // caret in it selects exactly that group's byte span.
    let text = "The committee met on Tuesday and the\nchair insisted on a vote. Then we left.\n";
    let mut h = Harness::new(text, None, (40, 10));
    { let mut ed = h.editor.borrow_mut(); ed.active_mut().view.ventilate = true;
      ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(5);
      crate::derive::rebuild(&mut ed); }
    h.render();
    // The block anchors one VentBlock; sentence 1 is one group (its first row carries the count).
    { let ed = h.editor.borrow();
      let vb = ed.active().view.vent_blocks.get(&0).expect("paragraph anchored at 0");
      assert!(matches!(vb.gutter.first(), Some(crate::ventilate::GutterCell::Count(_)))); }
    // select_sentence selects the whole hard-wrapped sentence (byte span across the newline).
    { let mut ed = h.editor.borrow_mut();
      crate::commands::run(crate::commands::Command::SelectScope(crate::commands::Scope::Sentence), &mut ed, &SharedClock::new(0)); }
    let ed = h.editor.borrow();
    let sel = ed.active().document.selection.primary();
    let picked = ed.active().document.buffer.slice(sel.from()..sel.to());
    assert!(picked.contains('\n') && picked.contains("committee") && picked.contains("vote"),
        "select_sentence grabs the whole hard-wrapped sentence the lens shows as one group: {picked:?}");
}

#[test]
fn e2e_toggle_off_is_byte_identical_preserves_caret_and_clears_vent_blocks() {
    let text = "Alpha one. Beta two. Gamma three.\n";
    let mut h = Harness::new(text, None, (30, 8));
    let before = { let ed = h.editor.borrow(); ed.active().document.buffer.slice(0..ed.active().document.buffer.len()) };
    h.alt('v'); // on
    { let ed = h.editor.borrow(); assert!(!ed.active().view.vent_blocks.is_empty(), "on → vent_blocks populated"); }
    { let mut ed = h.editor.borrow_mut(); ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(12); }
    h.alt('v'); // off
    let ed = h.editor.borrow();
    let after = ed.active().document.buffer.slice(0..ed.active().document.buffer.len());
    assert_eq!(before, after, "toggle on→off is byte-identical");
    assert_eq!(ed.active().document.selection.primary().head, 12, "caret preserved across toggle");
    // IMPORTANT 2: no stale resolver metadata after toggle-off; per-line geometry restored.
    assert!(ed.active().view.vent_blocks.is_empty(), "toggle-off clears vent_blocks (no stale metadata)");
    assert!(ed.active().view.line_layouts.contains_key(&0), "per-line entry for line 0 restored");
}
```

- [ ] **Step 2: Paint-origin + focus-window (indented + multi-line)** — the T-paint-origin /
  T-focus-window guardrails:

```rust
#[test]
fn e2e_focus_dims_right_rows_under_ventilate_indented() {
    // A 2-space-INDENTED, multi-line paragraph: focus-Sentence dims the non-focused sentence's
    // row-group on the RIGHT bytes (origin = ps, not line_start; a line_start origin would shift the
    // dim region by the indent and dim the wrong rows).
    let text = "  The committee met on a\nsunny day. It then voted twice.\n";
    let mut h = Harness::new(text, None, (30, 10));
    { let mut ed = h.editor.borrow_mut();
      ed.active_mut().view.ventilate = true;
      ed.view_opts.focus = true;
      ed.view_opts.focus_granularity = crate::config::FocusGranularity::Sentence;
      ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(text.find("committee").unwrap());
      crate::derive::rebuild(&mut ed); }
    h.render();
    let s1_row = (0..10u16).find(|&y| h.row(y).contains("committee")).unwrap();
    let s2_row = (0..10u16).find(|&y| h.row(y).contains("voted twice")).unwrap();
    assert!(h.dim_cols(s1_row).is_empty(), "focused sentence 1 row not dimmed");
    assert!(!h.dim_cols(s2_row).is_empty(), "sentence 2 row dimmed — origin-correct focus region");
}

#[test]
fn e2e_search_highlight_lands_on_right_bytes_in_multiline_ventilated_block() {
    // T-search-window / T-paint-origin (IMPORTANT 4 — NON-vacuous): a real search match on the SECOND
    // logical line of an INDENTED multi-line ventilated block must highlight the matched word's
    // columns. A raw-line_start origin (render.rs:530/652 unmigrated) would (a) window the match out
    // of the visible search set and/or (b) paint the highlight shifted by the indent — this asserts
    // the highlight lands on the actual "voted" glyphs.
    let text = "  The committee met on a\nsunny day. It then voted twice.\n";
    let mut h = Harness::new(text, None, (30, 10));
    { let mut ed = h.editor.borrow_mut(); ed.active_mut().view.ventilate = true;
      crate::derive::rebuild(&mut ed); }
    // Run a search for "voted" (mirror the e2e search-entry helper; grep `fn search`/`Msg::` in e2e.rs).
    h.search_for("voted"); // enters search mode, types the query, commits — see implementer note
    h.render();
    // Locate the row showing "voted" and assert its highlighted columns cover the word.
    let row = (0..10u16).find(|&y| h.row(y).contains("voted")).expect("match row visible");
    let hl = h.search_highlight_cols(row); // columns carrying the search-match style (see note)
    let line = h.row(row);
    let word_start = line.find("voted").unwrap() as u16;
    assert!(hl.contains(&word_start) && hl.contains(&(word_start + 4)),
        "search highlight covers the 'voted' glyphs at their real columns (origin-correct): hl={hl:?}, row={line:?}");
}
```

> **Implementer note (search harness):** confirm the e2e search-entry path and a highlight-probe
> helper: `grep -n "fn search\|Msg::Search\|search_bar\|Modifier::REVERSED\|hl_cols\|match_cols" wordcartel/src/e2e.rs wordcartel/src/render.rs`.
> If no `search_for`/`search_highlight_cols` helper exists, add them beside `dim_cols`
> (`e2e.rs:265`): `search_for` drives the real search Msg sequence (open, type, commit) the other e2e
> journeys use; `search_highlight_cols(y)` mirrors `underlined_cols`/`dim_cols` but tests the
> search-match style (the placed-path highlight face — confirm which `Modifier`/style
> `row_spans_placed` applies to a match, `render.rs` ~670-700). The pinned behavior is
> byte/column-correctness of the highlight, not the exact style constant.

- [ ] **Step 3: Composition (Source shows raw markers; measure narrows)**:

```rust
#[test]
fn e2e_ventilate_composes_with_source_shows_raw_markers() {
    let mut h = Harness::new("This is **bold** text here. Bye.\n", None, (50, 8));
    { let mut ed = h.editor.borrow_mut();
      ed.active_mut().view.ventilate = true;
      ed.active_mut().view.mode = crate::editor::RenderMode::SourcePlain; // all lines raw
      crate::derive::rebuild(&mut ed); }
    h.render();
    assert!((0..8u16).any(|y| h.row(y).contains("**bold**")),
        "ventilate + SourcePlain shows raw markers on the sentence row (L1/§6.1)");
}

#[test]
fn e2e_verbatim_active_line_reveals_raw_prose_stays_concealed() {
    // IMPORTANT 3 / §4.2: L1 (no raw reveal) is scoped to ventilated PROSE only. A VERBATIM block's
    // active line keeps today's raw reveal; a ventilated PROSE active line stays concealed-clean.
    // Doc: an active heading (verbatim) reveals "# ", while an active emphasis paragraph conceals "*".
    let text = "# Title here\n\nThis *word* matters. Bye now.\n";
    // (a) caret ON the heading (verbatim) → LivePreview reveals the raw "# ".
    let mut h = Harness::new(text, None, (40, 8));
    { let mut ed = h.editor.borrow_mut();
      ed.active_mut().view.ventilate = true;
      ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(2); // in "# Title"
      crate::derive::rebuild(&mut ed); }
    h.render();
    assert!((0..8u16).any(|y| h.row(y).contains("# Title")),
        "active verbatim heading reveals raw '# ' under ventilate (active_line stays effective)");
    // (b) caret IN the prose paragraph → the emphasis markers stay CONCEALED (L1, no active reveal).
    let mut h2 = Harness::new(text, None, (40, 8));
    { let mut ed = h2.editor.borrow_mut();
      ed.active_mut().view.ventilate = true;
      ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(text.find("word").unwrap());
      crate::derive::rebuild(&mut ed); }
    h2.render();
    assert!((0..8u16).any(|y| h2.row(y).contains("word matters")),
        "prose sentence renders concealed-clean");
    assert!(!(0..8u16).any(|y| h2.row(y).contains("*word*")),
        "active ventilated prose line stays concealed — no raw reveal at the caret (L1)");
}
```

- [ ] **Step 4: Run the guardrail suite** (one filter per invocation)

Run each separately: `cargo test -p wordcartel e2e_see_equals_select`,
`cargo test -p wordcartel e2e_toggle_off`, `cargo test -p wordcartel e2e_focus_dims_right_rows`,
`cargo test -p wordcartel e2e_search_highlight_lands`,
`cargo test -p wordcartel e2e_ventilate_composes_with_source`,
`cargo test -p wordcartel e2e_verbatim_active_line_reveals_raw`,
`cargo test -p wordcartel t_diff_spans`, `cargo test -p wordcartel t_perf`.
Expected: PASS.

> **Implementer notes:** (1) confirm the exact `Command`/`Scope` names for select-sentence
> (`grep -n "Scope::Sentence\|SelectScope\|select_sentence" wordcartel/src/commands.rs
> wordcartel/src/registry.rs`) — S5 registered `select_sentence` → `scope_range_at`'s
> `Scope::Sentence`; dispatch via the registry id `"select_sentence"` if the `Command` variant
> differs. (2) `SharedClock::new` is at `e2e.rs:831`; use it or the harness's clock. (3) If a wrap
> lands on a different row than assumed, locate rows by `h.row(y).contains(...)` rather than
> hard-coding indices — the pinned behavior is the byte-correctness, not the row number. (4) If focus
> dimming needs `no_color()`/`Depth::None` like the render focus tests, replicate that harness setup.

- [ ] **Step 5: Full gates + smoke**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets && cargo test -p wordcartel --test module_budgets && bash scripts/smoke/run.sh`
Expected: all green; module_budgets 5/5; quote the smoke one-line summary in the pre-merge report.

- [ ] **Step 6: Commit**

```bash
git add wordcartel/src/e2e.rs wordcartel/src/ventilate.rs
git commit -m "test(ventilate): SEE==SELECT, idempotence, paint-origin, focus-window, perf guardrails (S6)"
# + trailers
```

---

## Test-plan coverage map (spec §12 → tasks)

| Spec test | Where it lands |
|---|---|
| T-see-select (SEE==SELECT hard-wrapped) | Task 8 `e2e_see_equals_select_hard_wrapped_sentence` |
| T-idempotence (byte-identity + caret + vent_blocks cleared) | Task 8 `e2e_toggle_off_is_byte_identical_preserves_caret_and_clears_vent_blocks` |
| T-colmap-roundtrip (former-newline byte) | Task 5 `fill_produces_one_rowgroup_per_sentence_and_reflows_hard_wrap` |
| T-anchor-resolver (incl. `layout_line_active`) | Task 3 `resolver_resolves_interior_line_and_origin_is_line_start_when_off`; Task 4 `resolver_origin_matches_line_start_when_ventilate_off` |
| T-indent-origin (lens spans == select-sentence, indented) | Task 3 `t_indent_origin_lens_spans_equal_select_sentence_for_indented_paragraph` |
| T-diff-spans (== `sentence_spans` on RAW + verse fixture) | Task 8 `t_diff_spans_equals_sentence_spans_on_raw` |
| T-focus-window / T-paint-origin (focus dim, indented + multi-line) | Task 8 `e2e_focus_dims_right_rows_under_ventilate_indented` |
| T-search-window (NON-vacuous, real match, right columns) | Task 8 `e2e_search_highlight_lands_on_right_bytes_in_multiline_ventilated_block` |
| T-classify (prose vs verbatim) | Task 2 `classify_paragraph_vs_verbatim` |
| T-gather-newline (segment-raw-then-display-normalize) | Task 2 `segment_raw_preserves_hard_break_veto`, `display_normalizes_newline_length_preserving` |
| T-gutter (count, right-align, `999`, continuation, overwrite-no-shift) | Task 6 `e2e_gutter_shows_right_aligned_word_counts`, `e2e_gutter_clamps_to_999`, `e2e_gutter_overwrites_lead_in_no_content_shift` |
| T-verbatim-active-reveal (L1 scoped to prose) | Task 8 `e2e_verbatim_active_line_reveals_raw_prose_stays_concealed` |
| T-zero-term (one row-group, one count) | Task 5 (add a `sentence_spans("no terminator here").count()==1` assertion to the fill test) |
| T-key-rerun (`LayoutKey.ventilate`) | Task 1 `ventilate_flag_reruns_layout` |
| T-perf (visible-range, no doc walk) | Task 8 `t_perf_edit_relays_visible_range_only` |
| T-compose-source (Source raw markers) | Task 8 `e2e_ventilate_composes_with_source_shows_raw_markers` |
| T-registry (`toggle_ventilate` + palette gate) | Task 7 `toggle_ventilate_is_stateful_onoff_and_flips_the_flag` + `palette.rs` gate |
| T-chord (`alt-v` CUA / WordStar unbound) | Task 7 `ventilate_bound_alt_v_in_cua_unbound_in_wordstar` |

---

## Pipeline status

**Plan: AUTHORED (2026-07-13) — Codex plan gate round 1 (NO-GO) folded; re-entering the gate.**
**Not yet done:** Codex plan gate re-run → subagent-driven TDD execution → Fable whole-branch + Codex
pre-merge gates → `--no-ff` merge.

---

## History

- **2026-07-13 — Codex PLAN gate round 1 (NO-GO) folded** (1 Critical + 4 Important + 1 Minor; no
  approved decision F1–F5/L1–L7 or user-visible behavior changed — the Critical is a mechanism fix
  that PROTECTS SEE==SELECT). Verified each against real source first.
  - **CRITICAL — origin rebound to `ps` (`paragraph_range_at` start), not `block.span.start`.**
    Verified `render.rs:503-506`: `select-sentence`/focus segment over `paragraph_range_at`'s window
    `(ps, pe)` with origin `ps`, and `paragraph_range_at` (`nav.rs:655-685`) falls back to a physical
    `line_start`-based range when no block contains the caret — so `block.span.start` would diverge on
    fallback/indent cases. The lens now gathers over and offsets against the identical
    `paragraph_range_at` call; `role_at` is retained ONLY for prose-vs-verbatim classification.
    SEE==SELECT + focus-window-identity now hold BY CONSTRUCTION. Changed: header/task-list,
    `prose_block_at` doc, `VentBlock`/`Resolved`/`resolve`/`vent_block_range`/`origin_of`/
    `layout_block_on_demand` docs, `fill_visible` (`(ps, pe)` naming + `byte_origin: ps`), and
    **T-indent-origin strengthened to assert the lens's global spans EQUAL `select-sentence`'s** for an
    indented multi-line paragraph (not just the origin value).
  - **IMPORTANT 1 — gutter OVERWRITES the reserved lead-in (no double-count).** Verified
    `push_prefix_lead_in` (`render.rs:627-628`) emits `" ".repeat(prefix_width)` for a glyphless row.
    Task 6 now `splice(0..1, gutter)` over that single lead-in span (reserved width == painted width);
    glyph-carrying verbatim (list/blockquote) keeps reserve 0 (no glyph/cursor desync), a minor
    left-inset accepted as deferred residue (§5.4). Added `e2e_gutter_overwrites_lead_in_no_content_shift`
    (content begins at col 6, not 12).
  - **IMPORTANT 2 — clear `vent_blocks` on the non-ventilate rebuild path.** Verified `derive.rs:253`
    clears only `line_layouts`. The thin `derive` branch now clears `vent_blocks` on the else path
    (runs on the toggle-off gate miss). Toggle-off test asserts `vent_blocks.is_empty()` + per-line
    geometry restored.
  - **IMPORTANT 3 — L1 scoped to prose; verbatim keeps active raw-reveal.** The fill's verbatim arm
    now uses `line_render_for(mode, l == active_line)` (was `false`); prose uses `false`. Added
    `e2e_verbatim_active_line_reveals_raw_prose_stays_concealed`.
  - **IMPORTANT 4 — non-vacuous search test.** Split the old focus+search test; the new
    `e2e_search_highlight_lands_on_right_bytes_in_multiline_ventilated_block` sets a REAL match on the
    second line of an indented multi-line block and asserts the highlighted columns cover the word
    (would fail on unmigrated `render.rs:530/652` geometry).
  - **MINOR — one `cargo test` filter per command.** Split every multi-filter `cargo test -p wordcartel
    a b c` into separate invocations.
  - The spec's own round-1-plan-gate fold (Critical origin) is recorded in the spec's History.
- **2026-07-13 — Codex plan gate round 2 (sweep residue).** Swept stale current-fact phrasings the
  round-1 origin fold missed: the `layout_block` Interface signature + doc (`block_start`/
  "block-relative" → `ps`/"window-relative to `ps`"), FLAG 2 ("BLOCK-RELATIVE" → "WINDOW-RELATIVE to
  `ps`"), `segment_block` docs, Global-Constraint §3, and the "block-aware"/"span.start origin"
  descriptors in commit messages → "window-aware"/`ps`. No logic/decision change.
- **2026-07-13 — Codex plan gate round 3 (L1 Global-Constraint residue).** Grep-driven sweep
  (`conceal|raw reveal|ventilated block|markers? (stay|remain)|inactive`) of both docs. Fixed the LIVE
  over-broad L1 statement in Global Constraints §1 ("no raw reveal inside ventilated blocks" → "no raw
  reveal on ventilated PROSE rows — verbatim lines keep the active-line raw reveal"); likewise scoped
  the spec's L1 ruling summary (§2) and §6.1's LivePreview bullet. Every other body hit was already
  prose-scoped or a location/axis descriptor; dated History entries left untouched. No logic/decision
  change.

---

## FLAGS — decisions forced while turning the spec into concrete code (highest gate risk)

**FLAG 1 (plan-level seam choice — the spec explicitly deferred this to the plan, §3.4).**
**Gutter paint seam = `layout()` `reserve_cols` param (cursor correctness) + render-paints-cells
(glyphs).** The spec listed three candidate seams; this plan picks a hybrid: core `layout()` gains an
additive `reserve_cols: usize` that folds into `ColMap.prefix_width`, so ALL cursor math (screen_pos
`vcol`, `visual_to_source` clamp, hang-indent) is correct **by construction** with zero per-consumer
arithmetic; render then paints the actual gutter GLYPHS (count / `│` / blank) into those reserved
columns via a small `ventilate::gutter_span` helper (keeping `render.rs` under its 900 budget). The
alternative pure "render-paints-cells with a manual +6 everywhere" was rejected — it would re-derive
prefix offsetting at every cursor site. **Cost:** one additive param on `layout()` threaded to ~4
call sites (each passes `0` when off). No behavior change for non-ventilated callers. **Gutter paint =
OVERWRITE, not prepend** (round-1 fix): render `splice(0..1, gutter_span)` over the single 6-wide
lead-in `push_prefix_lead_in` emits for a glyphless prose row (`render.rs:627-628`), so reserved width
== painted width (verified; `e2e_gutter_overwrites_lead_in_no_content_shift` pins it). **Glyph-carrying
verbatim blocks (lists, blockquotes) get reserve 0** — their glyph lead-in is painted at natural width,
and a 6-col reservation would desync glyph paint from `prefix_width`; the resulting ~6-col left-inset
vs prose is accepted as deferred-verbatim residue (§5.4). Glyphless verbatim reserves 6 (blank gutter
via the untouched lead-in).

**FLAG 2 (representation choice — the spec blessed "widen the value OR a parallel map," §5.2).**
**Chose a parallel `View.vent_blocks: BTreeMap<usize, VentBlock>`** (window metadata: `last_line`,
`byte_origin` = `ps`, `gutter`) keyed by the window anchor line, leaving `line_layouts` tuple-typed.
This keeps the flag-OFF path's `(rows, map)` destructures **untouched** (minimal churn) while giving
the resolver its line-range + origin. `ColMap.src`/`src_span` are WINDOW-RELATIVE (to `ps`) for
ventilated entries (origin added by the resolver), LINE-RELATIVE otherwise — the single semantic
every Task 4 consumer respects. Cleared alongside `line_layouts` in `invalidate_layout` AND on the
non-ventilate rebuild path (IMPORTANT 2).

**FLAG 3 (low risk — task-ordering within the resolver keystone).** Task 3's `t_indent_origin` test
depends on Task 5's fill to populate `vent_blocks`; it is written RED in Task 3 and either
`#[ignore]`d until Task 5 or landed green when Tasks 3+5 are executed together. The per-line resolver
test (no ventilate) is fully green at Task 3. Noted so a strict task-by-task executor does not read
the dependency as a plan error.

**Non-flag confirmations (grounded 2026-07-13):** `alt-v` is FREE in CUA (`grep "alt-v"
keymap.rs` → no hit; only `alt-shift-v` = `move_block_to_scratch` taken); `derive.rs` is NOT a
budgeted hub (so the thin fill branch is safe; `render.rs` 900 is the live constraint, kept by
housing `gutter_span`/`layout_block` in `ventilate.rs`); `ColMap`/`Placed`/`VisualRow` fields are all
`pub` (shell can stitch a block ColMap); `count::word_count`, `sentence_spans`, `paragraph_range_at`,
`role_at`, `byte_to_line`/`line_to_byte`/`slice` all have the signatures used above; the e2e
`Harness` already carries `dim_cols`/`underlined_cols`/`row`/`alt(c)` (S5).
