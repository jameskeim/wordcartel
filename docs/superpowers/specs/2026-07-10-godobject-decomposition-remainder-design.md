# God-object decomposition remainder — H9 + H11 + H14 (design)

**Date:** 2026-07-10
**Status:** draft for Codex spec review
**Scope:** one effort / one branch, implemented in order H9 → H11 → H14.
**Ground truth:** `.git/sdd/briefs/map-h9.md`, `map-h11.md`, `map-h14.md` (verified against the
real source for this spec — every signature/behavior claim below was re-read from the files named);
item prose in `docs/engineering-health.md` (H9/H11/H14); rules in `CLAUDE.md` → Module structure.
All line numbers cited are *current-as-of-writing* and will drift — implementers anchor on symbol
names (`workspaceSymbol`/`documentSymbol`/grep), never recorded lines.

---

## 1. Goal & non-goals

**Goal.** Finish the H1-class god-object decomposition: leave `derive::rebuild_downstream`,
`commands::run`, and `render()` as thin dispatchers that delegate into focused domain modules,
per the anti-regrowth rule ("a match arm or loop iteration is a thin delegation into a domain
module — never an inline body"). Three items:

- **H9** — lift the four pure logical-line helpers (`total_logical_lines`, `line_start`,
  `line_text`, `line_render_for`) out of `wordcartel/src/derive.rs` into a new
  `wordcartel/src/lines.rs`; `derive.rs` keeps only the `rebuild`/`rebuild_downstream` pipeline,
  `LayoutKey`, and its test-only bench instrumentation. Call paths preserved via re-export.
- **H11** — decompose `commands::run` (`wordcartel/src/commands.rs`): (a) factor the repeated
  post-edit epilogue into ONE helper; (b) lift the 8 buffer-edit arm bodies into a
  `commands::edit` submodule, leaving those arms one-line delegations.
- **H14** — split the `render()` body (`wordcartel/src/render.rs`) into `paint_rows` /
  `paint_status` / `place_cursor` (plus the minimum supporting helpers required by the 100-line
  clippy gate), and unify the near-duplicate segs-vs-placed prefix lead-in and style ladder.

**The bar: behavior-identical.** Zero user-visible change. Every existing test passes
*unchanged* — no assertion is weakened or rewritten to fit. The only test edits permitted are
tests that legitimately MOVE with their subject (the ~11 logical-line unit tests → `lines.rs`,
§8.1). Golden render tests must produce byte-identical cell grids.

**Non-goals (explicitly out of scope):**
- No new commands, options, keybindings, palette/menu entries, or command semantics changes (§6).
- No dedup of the Undo/Redo caret-snap tail, no lifting of `Move`/`Copy`/`Paste`/`Save`/`Quit`/
  selection arms out of `run` — the map is explicit that `Quit` is not an edit and `Move`'s
  pre-dispatch side effects (sel_history clear, jump-ring push) belong with the arm
  (map-h11 §2/§7). `run` keeps its item-local `#[allow(clippy::too_many_lines)]` with an updated
  rationale (§4.5).
- No "fixing" of oddities preserved for byte-identical behavior (e.g. `render()`'s
  `sort_unstable` over already-sorted `BTreeMap` keys, §5.2; the per-path recomputation of
  `line_off`).
- No H13/H10/H7 work (Editor field clustering, reduce-chain table, unwrap audit) — separate items.
- No `cargo fmt`; all new/moved code hand-matched to neighbors (dense house style, em-dash in
  prose comments).

---

## 2. Sequencing rationale — H9 → H11 → H14

Risk-ascending order, and each item reduces churn for the next:

1. **H9 — trivial risk.** A verbatim move of four pure functions plus a re-export; the re-export
   means the ~50 crate-wide `derive::line_*`/`derive::total_logical_lines` call sites — including
   the ones inside the code H11 and H14 subsequently move — do not change at all. Landing it first
   means later diffs never race it.
2. **H11 — low risk.** Verbatim body moves guarded by buffer-STATE tests: the 52 `commands.rs`
   tests assert `buffer.to_string()`, `nav::head`, `CommandResult`, `dirty()`/`version` — not
   rendered cells (map-h11 §6) — so a behavioral slip fails loudly and legibly.
3. **H14 — highest risk.** The gate is ~98 per-cell symbol+style assertions across 83 `render.rs`
   tests + 25 `e2e.rs` journeys driving the real `render()` through `Terminal<TestBackend>`
   (map-h14 §7). Any span-boundary or paint-order change breaks goldens. Done last, on a settled
   branch, with the whole suite as the oracle.

The stated dependency "H14 depends on H9+H11 having landed" is sequencing discipline (shared-branch
churn + the H9 re-export guarantee), not a compile-order dependency; H14's moved code keeps its
`derive::line_start` call path, which H9's re-export keeps valid forever.

---

## 3. H9 — `lines.rs`: pure logical-line helpers

### 3.1 What moves (verbatim, with their doc comments)

From `wordcartel/src/derive.rs` (current locations: `line_render_for` :25, `total_logical_lines`
:91, `line_start` :104, `line_text` :116) into new file `wordcartel/src/lines.rs`:

```rust
pub(crate) fn line_render_for(mode: crate::editor::RenderMode, is_active_line: bool)
    -> wordcartel_core::style::LineRender
pub fn total_logical_lines(buf: &TextBuffer) -> usize
pub fn line_start(buf: &TextBuffer, line: usize) -> usize
pub fn line_text(buf: &TextBuffer, line: usize) -> String
```

Signatures unchanged. All four are pure (map-h9 §1, re-verified): `total_logical_lines` calls only
`buf.snapshot().len_lines()`; `line_start` calls `total_logical_lines` + `buf.line_to_byte` +
`buf.len`; `line_text` calls `line_start` + `total_logical_lines` + `buf.slice`; `line_render_for`
is a `match` on `RenderMode`. None touch `LayoutKey`, caches, `block_tree`, or `layout` — no
circular import (`lines.rs`'s deps do not point back at `derive`).

`lines.rs` imports: `use wordcartel_core::buffer::TextBuffer;` — that is the only `use` needed;
`crate::editor::RenderMode` and `wordcartel_core::style::LineRender` are path-qualified in
`line_render_for`'s signature exactly as they are today. Module doc comment: pure logical-line
helpers over `TextBuffer` — line count, line-start offset, line text, and the render-mode →
`LineRender` mapping; no derive-pipeline dependence.

### 3.2 What stays in `derive.rs`

Everything else (map-h9 §4): the `Editor`/`block_tree`/`layout` imports, `LayoutKey`, the
`#[cfg(test)]` `LAYOUT_RUNS`/`HEADING_STARTS_WALKS` thread-locals and `bench_spans` module,
`rebuild`, `full_parse_phase`, `rebuild_downstream`, `apply_parse_result`, and the remaining test
module. The `use wordcartel_core::buffer::TextBuffer;` import in `derive.rs` is removed IF nothing
left references the name (the moved fns and the moved tests' `buf` helper were its users; the
warning-free `cargo build` gate is the arbiter — an unused-import warning fails the gate).

### 3.3 Registration and re-export

- **Registration:** `pub mod lines;` in `wordcartel/src/lib.rs`, inserted beside `pub mod derive;`
  (lib.rs:5).
- **Re-export in `derive.rs`** (top of file, after the `use` block) — this is the *durable* public
  call path, not a transition shim; it is what keeps the effort behavior- and API-identical:

```rust
pub use crate::lines::{line_start, line_text, total_logical_lines};
pub(crate) use crate::lines::line_render_for;
```

  `line_render_for` is `pub(crate)` (nav.rs:64 calls `crate::derive::line_render_for`), and a
  re-export cannot widen visibility, so it needs the separate `pub(crate) use`.
- **Bare-name call sites inside `derive.rs`:** `rebuild_downstream` calls all four helpers
  unqualified (`total_logical_lines(buf)`, `line_text(buf, l)`,
  `blocks().role_at(line_start(buf, l))`, `line_render_for(b_mode, …)`). The `use` re-export above
  brings the names into `derive`'s scope, so these bare calls compile unchanged — no edits inside
  `rebuild_downstream`.
- **All external call sites unchanged:** `nav.rs` (all four, ~30 sites), `render.rs`
  (`line_start` ×4), `commands.rs` + `prompts.rs` (`total_logical_lines`) keep their
  `derive::`-qualified paths via the re-export. (Grounded correction: eng-health H9 prose lists
  `transform.rs` as a `line_start` consumer; grep confirms the map — `transform.rs` only contains
  its own `extend_to_line_start`, a false positive. No transform.rs edit.)

### 3.4 Tests that move (see §8.1)

The "Logical-line edge-case helpers" banner block in `derive.rs`'s `mod tests` — the local
`fn buf(s: &str) -> TextBuffer` helper plus 11 tests (`total_lines_empty_is_one`,
`total_lines_no_newline`, `total_lines_trailing_newline_is_two`, `total_lines_lone_newline`,
`total_lines_two_lines_no_trailing_newline`, `line_start_positions`, `line_text_strips_newline`,
`line_text_empty_buffer`, `line_text_no_trailing_newline`, `line_text_lone_newline`,
`line_text_multibyte`) — moves verbatim into a `#[cfg(test)] mod tests` in `lines.rs` with
`use super::*;` (which supplies the four fns and, via `lines.rs`'s own import, `TextBuffer`).
`unicode_line_breaks_do_not_split_logical_lines` STAYS in `derive.rs` (it drives a full
`Editor`/rebuild and calls `crate::derive::total_logical_lines` — still valid via the re-export).
`line_render_for` has no dedicated test; its coverage (`active_line_renders_raw`,
`caret_on_phantom_line_conceals_last_content_line`) stays in `derive.rs` untouched.

### 3.5 Resulting seam

`derive.rs` = the recompute pipeline only (`rebuild`/`rebuild_downstream`/`LayoutKey`), a thin
consumer of `lines.rs`. New line-space utilities go in `lines.rs`; the pipeline never re-absorbs
them.

---

## 4. H11 — decompose `commands::run`

### 4.1 Current shape (verified against source)

`pub fn run(cmd: Command, editor: &mut Editor, clock: &dyn Clock) -> CommandResult`
(commands.rs:211) — a free fn, one exhaustive flat `match cmd` with 20 arms and no `_`, under
`#[allow(clippy::too_many_lines)]` (:210). `CommandResult = Handled | Noop | Quit` (:92–99).
Mutation goes through `editor.apply(txn, edit, EditKind, clock)`. There is no
`Executor`/`msg_tx`/`submit_transaction` in `run` (map-h11 §0 drift corrections, confirmed).

### 4.2 The shared epilogue helper — edit arms ONLY

The epilogue is NOT uniform across `run` (the key hazard, map-h11 §2). The exact verbatim tail on
the 8 buffer-edit arms — and only them — is:

```rust
derive::rebuild(editor);
nav::ensure_visible(editor);
editor.active_mut().desired_col = None;
CommandResult::Handled
```

It appears at **12 sites**: twice each in `InsertChar`/`InsertNewline`/`Backspace`/`DeleteForward`
(the sel-vs-collapsed pre-branch duplicates it), once each in `Cut`/`DeleteWord`/`DeleteLine`/
`DeleteToLineEnd`. The helper (private to the new `commands/edit.rs`, §4.3):

```rust
/// Post-edit epilogue shared by every buffer-edit primitive: re-derive, re-scroll,
/// re-anchor vertical motion. Edit paths ONLY — Move/CycleRenderMode keep rebuild+
/// ensure_visible without the desired_col reset, Undo/Redo/ShrinkSelection insert a
/// caret-snap first, SelectAll doesn't rebuild; those arms keep their own tails.
fn settle_after_edit(editor: &mut Editor) -> CommandResult {
    derive::rebuild(editor);
    nav::ensure_visible(editor);
    editor.active_mut().desired_col = None;
    CommandResult::Handled
}
```

Design decisions, each demanded by the maps:
- **The helper deliberately does NOT wrap `editor.apply`.** `Cut` splices
  `clipboard_sync_request = …` between `apply` and `rebuild`; keeping `apply` in the caller gives
  every site the same shape — `editor.apply(txn, edit, kind, clock); …optional splice…;
  settle_after_edit(editor)` — with ONE helper covering all 12 sites including Cut. An
  apply-wrapping variant would need a second helper or a parameter just for Cut.
- **Borrow safety:** `fn settle_after_edit(editor: &mut Editor)` is called after every read borrow
  (`sel`, `doc_len`, `buf_snap`) has dropped and `txn`/`edit`/`cs` have been moved into `apply` —
  exactly the borrow-safe shape the map prescribes (map-h11 §7.5).
- **Arms that resemble but differ get NOTHING:** `Move` (rebuild+ensure_visible, no desired_col),
  `Undo`/`Redo` (rebuild → `place_caret_visible(…, CaretPlace::SnapOut)` caret-snap →
  ensure_visible → desired_col), `CycleRenderMode` (no desired_col), `ShrinkSelection`
  (rebuild+snap+ensure, no desired_col), `SelectAll` (no rebuild), `SelectScope`/`ExpandSelection`
  (re-derive via the existing private `set_selection_range`), `Copy`/`Paste`/`Save`/`Quit`
  (no epilogue). All stay verbatim in `run`.

### 4.3 The `commands::edit` submodule

New file `wordcartel/src/commands/edit.rs`, registered by a private `mod edit;` in `commands.rs`
(after the `use` block). Rust allows `commands.rs` + a `commands/` subdirectory, so no
`commands/mod.rs` conversion and zero path churn — everything currently at `crate::commands::…`
stays put. The 8 edit-arm bodies move verbatim (sel-branch AND collapsed-branch, all early `Noop`
guards, all comments), with only the epilogue lines replaced by `settle_after_edit(editor)`:

```rust
pub(super) fn insert_char(editor: &mut Editor, c: char, clock: &dyn Clock) -> CommandResult
pub(super) fn insert_newline(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
pub(super) fn backspace(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
pub(super) fn delete_forward(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
pub(super) fn cut(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
pub(super) fn delete_word(editor: &mut Editor, back: bool, clock: &dyn Clock) -> CommandResult
pub(super) fn delete_line(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
pub(super) fn delete_to_line_end(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
```

- **Return type:** every fn returns `CommandResult` — required because the bodies contain early
  `Noop` returns (Backspace `head == 0`, DeleteForward `next == head`, DeleteWord `from == to`,
  DeleteLine `len == 0` and `from == to`, DeleteToLineEnd `head >= to`, Cut empty-selection) and
  early `Handled` returns (the four sel-branches).
- **`pub(super)`** — visible to `commands` only; nothing outside `commands` calls the primitives
  today (everything routes through `run`). Widening later (e.g. Effort P wanting direct
  primitives) is a deliberate one-line act.
- **`replace_changeset` stays private in `commands.rs`, unmoved.** Rust privacy is
  module-and-descendants, so `commands::edit` reaches it as `super::replace_changeset` — no
  visibility change. (It is also used by `build_range_replace`, which must not move; keeping the
  builder cluster together in `commands.rs` is deliberate.)
- **EditKind preserved exactly:** collapsed `InsertChar` uses `EditKind::Type`; every other apply
  in the module uses `EditKind::Other` (including the InsertChar sel-branch and InsertNewline both
  branches — the newline `EditKind::Other` coalescing-break comment moves with the code).
- **Cut's splice preserved:** `editor.apply(…); if let Some(text) = editor.register.get()… {
  editor.clipboard_sync_request = Some(text); } settle_after_edit(editor)`.
- **Imports `edit.rs` needs** (from map-h11 §5, verified): `crate::derive`, `crate::nav`,
  `crate::editor::Editor`, `super::{replace_changeset, CommandResult}`,
  `wordcartel_core::block_tree::Edit`, `wordcartel_core::change::ChangeSet`,
  `wordcartel_core::history::{Clock, EditKind, Transaction}`,
  `wordcartel_core::selection::Selection`, `wordcartel_core::register` (Cut). DeleteLine/
  DeleteToLineEnd call `derive::total_logical_lines` — valid via the H9 re-export, unchanged.

### 4.4 What `run` becomes

The 8 edit arms become one-liners:

```rust
Command::InsertChar(c)          => edit::insert_char(editor, c, clock),
Command::InsertNewline          => edit::insert_newline(editor, clock),
Command::Backspace              => edit::backspace(editor, clock),
Command::DeleteForward          => edit::delete_forward(editor, clock),
Command::Cut                    => edit::cut(editor, clock),
Command::DeleteWord { back }    => edit::delete_word(editor, back, clock),
Command::DeleteLine             => edit::delete_line(editor, clock),
Command::DeleteToLineEnd        => edit::delete_to_line_end(editor, clock),
```

The other 12 arms (`Move`, `Copy`, `Paste`, `Undo`, `Redo`, `CycleRenderMode`, `Save`, `Quit`,
`SelectScope`, `ExpandSelection`, `ShrinkSelection`, `SelectAll`) are byte-for-byte untouched.
`run`'s signature, the `Command`/`CommandResult`/`Scope`/`Dir` enums, and the module's public
builders are all unchanged. The module doc comment (commands.rs:1–11, which narrates the
edit-command epilogue) is updated to name `commands::edit` + `settle_after_edit` as where steps
3–6 now live — a doc-accuracy edit, not a behavior change.

### 4.5 `run`'s size and the retained `#[allow]`

Post-refactor `run` is ≈240 lines (471 − ~232 lines of moved edit bodies + 8 delegation lines) —
still over the 100-line clippy threshold because 12 non-edit arms keep their (small, cohesive)
bodies by design (§1 non-goals). The item-local allow stays with an updated rationale, e.g.:
`#[allow(clippy::too_many_lines)] // exhaustive flat Command dispatch — edit arms delegate to
commands::edit; the remaining arms are small non-edit state ops (see H11)`. This matches the house
counter-caveat ("a long function is fine when it is a genuinely flat, cohesive dispatch … mark it
with a reasoned allow").

### 4.6 Public surface stability (external callers)

Unchanged and un-moved, staying `pub` in `commands.rs` at their current paths:
- `build_multi_replace` — callers: `blocks_marked.rs`, `derive.rs` (tests), `search_ui.rs`,
  `scratch.rs`, `editor.rs`, `workspace.rs`, `session_restore.rs`.
- `build_range_replace` — callers: `search_ui.rs` (×4), `jobs_apply.rs` (×2), `transform.rs`.
- `scope_range_at` — callers: `mouse.rs` (×2).
Also unchanged: private `scope_range`, `set_selection_range`. External `run` callers stay green
with no edits: `input.rs:58`, `registry.rs:699` (adapter), `app.rs:335`.

### 4.7 Resulting seam

`run` = thin exhaustive dispatch; buffer-edit behavior lives in `commands/edit.rs`. A new edit
primitive = a new `edit.rs` fn + one delegation arm (the compiler's exhaustive-match check forces
the arm); `run` never regrows an inline edit body. Effort P's plugin-invoked edits route through
`run` → `edit::*` unchanged.

---

## 5. H14 — split `render()` by paint surface + segs/placed unification

### 5.1 Current shape (verified against source)

`pub fn render(frame: &mut Frame, editor: &mut Editor)` (render.rs:216), ~522-line body under
`#[allow(clippy::too_many_lines)]` (:215), in 13 phases (map-h14 §1): tiny-terminal guard → layout
scalars → `tg = nav::text_geometry(editor)` (ONE call) → opaque-canvas fill → wrap-guide →
focus-region → row-data snapshots → ROW LOOP → `ChromeStyles::build` → scrollbar → STATUS →
CURSOR → `render_overlays::paint`. `nav::text_geometry(editor: &Editor) -> TextGeometry`
(`pub struct TextGeometry { pub text_left: u16, pub text_width: u16 }`, nav.rs:18). The row loop
reads `editor.active().view.line_layouts: BTreeMap<usize, (Vec<VisualRow>, ColMap)>`
(editor.rs:117; `VisualRow`/`ColMap`/`Placed` in `wordcartel-core/src/layout.rs`).

### 5.2 New function inventory — exact signatures

The mandate names three painters; the 100-line `clippy::too_many_lines` gate forces the row-paint
surface to be more than one fn, so it decomposes into a snapshot-gatherer + a loop driver + two
span builders + the two unification helpers. All new items are private, placed ABOVE the
`#[cfg(test)] mod tests` boundary (they are production lines for the `module_budgets` count).

```rust
/// Everything the row loop reads that is snapshotted once per frame (render() phases
/// 6–7 today): focus region, sorted layout keys, search window, diagnostics, selection,
/// marked block, and the once-hoisted use_placed/plain_source selectors.
struct RowCtx<'a> {
    scroll: usize,
    focus_region: Option<(usize, usize)>,
    sorted_lines: Vec<usize>,
    hl_current: Option<wordcartel_core::search::Match>,
    hl_window: Vec<wordcartel_core::search::Match>,
    diag_all: &'a [wordcartel_core::diagnostics::Diagnostic],
    sel_from: usize,
    sel_to: usize,
    has_sel: bool,
    marked_block: Option<crate::editor::MarkedBlock>,
    use_placed: bool,
    plain_source: bool,
}
```

`RowCtx` holds **exactly the fields `paint_rows`/`row_spans_segs`/`row_spans_placed` read — 12 of
them** (listed above): `scroll`, `focus_region`, `sorted_lines`, `use_placed` (read by
`paint_rows`); `plain_source` (both span builders); `diag_all`, `hl_current`, `hl_window`,
`marked_block`, `has_sel`, `sel_from`, `sel_to` (`row_spans_placed`). Three values that the real
code computes at gather time — `has_block` (render.rs:345), `block_hidden` (:344), and
`diag_active` (:332) — are ONLY inputs to the retained fields (`has_block`/`block_hidden` →
`use_placed`; `diag_active` → `diag_all`); the paint path never reads them (it paints selection
from `marked_block`, not `has_block`, and reads `diag_all`, not `diag_active`). Retaining any of
the three as a struct field would be a written-but-never-read field → dead-code warning → fails the
warning-free gate. So `gather_row_ctx` computes all three as **gather-time locals**, uses them to
derive `use_placed`/`diag_all`, and drops them; only the 12 read fields survive into `RowCtx`.

```rust
/// Snapshot the row loop's inputs (verbatim phases 6–7: focus_region, sorted_lines,
/// hl_current/hl_window, diag_active→diag_all, sel_*, marked_block, use_placed,
/// plain_source — same expressions, same order; has_block/block_hidden/diag_active
/// are gather-time locals that feed use_placed/diag_all and are not retained).
fn gather_row_ctx(editor: &Editor) -> RowCtx<'_>

/// Phase 8: the visible-row paint loop (owns screen_row, the outer/inner loop,
/// row_dim, fold_marker_n, the segs/placed selector, fold-marker insert, and the
/// per-row render_widget).
fn paint_rows(frame: &mut Frame, editor: &Editor, area: Rect,
              edit_top: u16, edit_height: u16, tg: &nav::TextGeometry)

/// Segs fast path — true no-op rows (no per-glyph styling).
fn row_spans_segs(editor: &Editor, ctx: &RowCtx, vr: &VisualRow, map: &ColMap,
                  row_dim: bool) -> Vec<Span<'static>>

/// Placed path — per-glyph selection/search/diag/marked-block styling with
/// run accumulation.
fn row_spans_placed(editor: &Editor, ctx: &RowCtx, l: usize, row_index: usize,
                    vr: &VisualRow, map: &ColMap, row_dim: bool) -> Vec<Span<'static>>

/// UNIFICATION #1 — the prefix lead-in shared by both span builders: row 0 paints the
/// real glyph (heading inverted-numeral box = REVERSED glyph + NORMAL space, else the
/// dim single-span glyph); continuation rows of a prefixed line push a prefix_width
/// blank spacer.
fn push_prefix_lead_in(spans: &mut Vec<Span<'static>>,
                       theme: &wordcartel_core::theme::Theme,
                       depth: wordcartel_core::theme::Depth,
                       vr: &VisualRow, map: &ColMap, row_dim: bool)

/// UNIFICATION #2 — the shared 4-arm style ladder, keyed on the inline `Style` VALUE
/// (seg.style on the segs path, p.style on the placed path):
///   row_dim && plain_source  → compose[Text, FocusDim]
///   row_dim && !plain_source → compose[Text, role, style, FocusDim]   (4-element)
///   !row_dim && plain_source → compose[Text]
///   else                     → compose[Text, role, style]
fn ladder_style(theme: &wordcartel_core::theme::Theme, depth: wordcartel_core::theme::Depth,
                role: wordcartel_core::style::BlockRole, inline: wordcartel_core::style::Style,
                row_dim: bool, plain_source: bool) -> RStyle

/// Phase 11: the status line (search bar / minibuffer / prompt / normal / calm-hidden
/// selection, right-flush Ln/Col/words composition, full-row set_style THEN Paragraph).
fn paint_status(frame: &mut Frame, editor: &Editor, area: Rect, status_row: u16,
                cs: &ChromeStyles)

/// Phase 12: the hardware cursor (search-field / minibuffer / normal-caret arms,
/// char-count column math, D2 col clamp to tg.text_width).
fn place_cursor(frame: &mut Frame, editor: &Editor, area: Rect, edit_top: u16,
                edit_height: u16, status_row: u16, tg: &nav::TextGeometry)
```

Notes on the surface:
- All new fns take `editor: &Editor` — every extracted phase only reads; `render()` reborrows its
  `&mut Editor` immutably per call. (`render_overlays::paint` at the end still gets the `&mut`.)
  `render_status::status_left_text`/`word_count_segment`, `chrome::status_line_visible`,
  `nav::head`/`screen_pos` all take `&Editor` (verified), so no signature friction.
- `Span<'static>` is correct: every span pushed today is built from an owned `String`
  (`glyph.clone()`, `seg.text.clone()`, `" ".repeat(…)`, `std::mem::take(&mut run)`).
- Argument counts stay ≤7 (clippy `too_many_arguments` threshold): `place_cursor` and
  `row_spans_placed` sit at exactly 7; `w` is derived from `area.width` inside `paint_status`/
  `place_cursor` rather than passed separately.
- `hl_current: Option<Match>` and `marked_block: Option<MarkedBlock>` are stored by value in
  `RowCtx` exactly as the loop uses them today (both are used by-value inside the per-glyph loop,
  so both are `Copy` — `MarkedBlock { start, end, hidden }`, editor.rs:123).
- New file? **No** — all of this stays in `render.rs`. The H14 mandate is a within-file body split
  (the `module_budgets` budget was set anticipating exactly this, see its comment "restructures
  within this file, budget holds"); moving painters to a new module is out of scope.

### 5.3 What `render()` becomes, and paint ordering (load-bearing)

```rust
pub fn render(frame: &mut Frame, editor: &mut Editor) {
    …area/w/h…; …tiny-terminal guard (early return)…;
    …menu_rows/edit_height/edit_top/status_row…;
    let tg = crate::nav::text_geometry(editor);          // THE one call
    …opaque canvas fill (verbatim)…;
    …wrap-guide (verbatim)…;
    paint_rows(frame, editor, area, edit_top, edit_height, &tg);
    let cs = ChromeStyles::build(&editor.theme, editor.depth, editor.canvas);
    …scrollbar (verbatim, uses cs)…;
    paint_status(frame, editor, area, status_row, &cs);
    place_cursor(frame, editor, area, edit_top, edit_height, status_row, &tg);
    crate::render_overlays::paint(frame, editor, &cs);
}
```

- **Ordering preserved exactly** (map-h14 §8.2): canvas → wrap-guide → rows (text overwrites the
  guide) → scrollbar (over rows) → status full-row `set_style` then its `Paragraph` →
  cursor placement (after status; both write `status_row`) → overlays LAST. All painters take
  `frame: &mut Frame` and run sequentially — nothing returns buffers to paint later.
- **`tg` single-call invariant** (map-h14 §8.3): computed once in `render()`, passed `&tg` into
  `paint_rows` and `place_cursor`; neither may call `text_geometry` again. `gather_row_ctx` does
  not need it.
- **`screen_row`** becomes a local of `paint_rows` (verified: no reads after the row loop).
- **`scroll`** is read inside `gather_row_ctx` (`editor.active().view.scroll`) — same value as the
  phase-2 read today (nothing mutates the editor between; the scrollbar's own
  `editor.active().view.scroll` read is untouched).
- With the body split out, `render()` is ≈90 production lines → **the
  `#[allow(clippy::too_many_lines)]` on `render` (:215) is REMOVED** — the H1-follow-up comment it
  carries is now discharged. Contingency if it lands marginally over 100: hoist the wrap-guide
  block into a `fn paint_wrap_guide` rather than re-adding the allow.

### 5.4 Moving phases 6–7 into `gather_row_ctx` is behavior-identical

The focus-region computation and the row-data snapshots are pure reads (no `Frame` writes, no
editor mutation, no interior-mutability touch — `active_fold_view()`'s `RefCell` memo is used only
by the scrollbar and `fold_marker_for`, both unmoved in position). Today no painting happens
between the wrap-guide and the row loop, so relocating these reads across that gap changes no
observable state. The expressions themselves move verbatim, in order, including the `diag_active`
gating (`RowCtx.diag_all` is `&[]` when diagnostics are stale — same as today) and the ONE-shot
`use_placed = !hl_window.is_empty() || diag_active || has_sel || has_block` /
`plain_source = mode == SourcePlain` hoists (map-h14 §8.4).

### 5.5 The row loop body — what is shared vs path-specific

`paint_rows`'s per-row body keeps verbatim: the `l < scroll` skip, `skip_rows`
(`view.scroll_row` on the first line), the `screen_row >= edit_height` `break 'outer`, `row_dim`
(focus-region overlap via `row_is_active`), `fold_marker_n` (`fold_marker_for` on
`row_index == skip_rows`), the selector
`let spans = if !ctx.use_placed { row_spans_segs(…) } else { row_spans_placed(…) }`, the
fold-marker insert (`"▸ "` at index 0 + the `"  … {n} lines"` DIM tail — applied to BOTH paths'
output, after span building), and the `render_widget` at
`Rect::new(area.x + tg.text_left, edit_top + screen_row, tg.text_width, 1)`.

**`row_spans_segs`** (from :395–450): `push_prefix_lead_in(…)`, then
`for seg in &vr.segs { let style = ladder_style(…, vr.role, seg.style, row_dim, ctx.plain_source);
let style = text_fg_or_base(style, &editor.theme, editor.depth); push Span::styled(seg.text.clone(), style) }`.
No per-glyph styling — the true no-op fast path stays a plain `vr.segs` iteration.

**`row_spans_placed`** (from :451–587): computes its own
`line_off = derive::line_start(buf, l)` and `lo`/`hi` (verbatim — the per-path recomputation is
kept, §1 non-goals), the diag windowing (upper-bound `partition_point` + linear `end > lo` filter),
`push_prefix_lead_in(…)`, then the per-glyph loop over `map.placed.iter().filter(|p| p.row ==
row_index)` verbatim: `ladder_style(…, vr.role, p.style, …)` → MarkedBlock patch (below
Selection) → Selection patch → SearchCurrent/SearchMatch patch → diag underline
(`add_modifier` + `underline_color`) → **`text_fg_or_base` applied BEFORE the run-accumulation
comparison** (so plain runs coalesce into one span — moving it changes span boundaries and thus
cell grids; map-h14 §8.5) → the run-flush on style change → the final flush. The
**run-accumulation flush is NOT unified or restructured**.

### 5.6 The unification — exactly what is shared, and why it is byte-identical

The two duplicated fragments differ today ONLY in the vec they push to (`segs_spans` vs
`hl_spans`) and the `Style` source field (`seg.style` vs `p.style`) — re-verified by side-by-side
read of :404–432 vs :477–503 (prefix) and :434–446 vs :515–527 (ladder):

1. **Prefix lead-in → `push_prefix_lead_in`.** Both copies: if `vr.prefix_glyph` is Some —
   `pe = prefix_element(vr.role)`; if `theme.heading_level_glyph` and the role is `Heading(n)`,
   push `HEADING_GLYPHS[(n.clamp(1,6)-1) as usize]` with
   `base.add_modifier(Modifier::REVERSED)` + a NORMAL-`base` space, where `base` is
   `compose[pe, FocusDim]` on dim rows else `compose[pe]`; otherwise push the glyph with
   `compose[pe, FocusDim]` on dim rows else `compose[pe].add_modifier(Modifier::DIM)`. If
   `prefix_glyph` is None and `map.prefix_width > 0`, push `Span::raw(" ".repeat(prefix_width))`.
   One code path, two call sites → identical spans by construction.
2. **Style ladder → `ladder_style`**, keyed on the `Style` value (map-h14 §2's "closure keyed on a
   Style value" realized as a fn — same thing, clearer for the dense house style and reusable by
   both builders without capture gymnastics). The four compose stacks are listed in §5.2; the dim
   non-plain arm is the **distinct 4-element compose `[Text, role, style, FocusDim]`** — it is NOT
   rewritten as `compose(…).add_modifier(DIM)` (map-h14 §8.6; the §13.2 FIX-1 comment moves with
   it).
3. **NOT unified:** the glyph iteration (segs iterate `vr.segs`; placed filters `map.placed` by
   row — different collections, different downstream styling; a shared iterator is explicitly the
   wrong target, map-h14 §8.7), the run flush, the fold-marker insert, `text_fg_or_base` placement
   (segs: per-seg immediately after the ladder; placed: after patches, before run comparison —
   both preserved at their current positions).

Because both helpers are verbatim factorings of code that was already textually identical modulo
the two parameters, every `compose` call sequence, modifier, and span boundary is unchanged →
byte-identical cell grids.

### 5.7 `module_budgets` arithmetic

`render.rs` production lines (before `mod tests`) ≈756 today; budget ≤900. The split adds fn
signatures/doc comments/`RowCtx` (≈ +60) and removes the duplicated prefix (~28 lines) + ladder
(~13 lines) copies (≈ −40); net estimate ≈ 775 ± 25 — comfortably under budget, consistent with
the constraint that helper bodies count as production and the duplication removal nets roughly
flat.

### 5.8 Resulting seam

`render()` = the frame skeleton: guard, geometry, canvas/guide, then five delegations
(`paint_rows`, `ChromeStyles::build`+scrollbar, `paint_status`, `place_cursor`,
`render_overlays::paint`). New paint behavior lands in the named painter (or a new one called from
the skeleton), never inline in `render()` — enforced by the removed allow (any regrowth past 100
lines now FAILS clippy) plus the 900-line hub budget.

---

## 6. Command-surface-contract conformance

Per `docs/design/command-surface-contract.md` (this statement is required because H11 touches the
command *dispatcher*):

**H11 refactors the dispatcher's internals and adds/removes/changes NO command.** The `Command`
enum, `CommandResult`, `run`'s signature and dispatch semantics, the registry, palette, menu, and
keybinding hints are all byte-identical in behavior. No user-settable option is added or moved; no
setter changes. The contract's invariant tests (palette-completeness,
every-option-has-a-command, hint re-resolution) run unchanged in the merge gate. The public
builders **`build_range_replace` and `build_multi_replace` (and `scope_range_at`) stay `pub` in
`commands.rs` at their existing paths** — re-export-stable for their external callers
(`search_ui`, `jobs_apply`, `transform`, `blocks_marked`, `scratch`, `editor`, `workspace`,
`session_restore`, `mouse`; §4.6). The `commands::edit` module is `pub(super)` plumbing behind
`run`, not a new command surface.

**H9 and H14: N/A — they do not touch the command surface** (pure line-math helpers and the
paint path respectively; no commands, options, palette/menu/hint code involved).

---

## 7. Module-structure / anti-regrowth conformance (per item)

| Item | Thin dispatcher (closed) | Domain module (open) | Enforcement |
|---|---|---|---|
| H9 | `derive::rebuild_downstream` — pipeline only, calls into `lines` | `lines.rs` — logical-line math | review; `derive.rs` sheds ~110 lines |
| H11 | `commands::run` — exhaustive flat dispatch, edit arms = 1-line delegations | `commands/edit.rs` — buffer-edit primitives + `settle_after_edit` | retained reasoned allow on `run` (§4.5); new edit behavior = new `edit.rs` fn + compiler-forced match arm |
| H14 | `render()` — ≈90-line frame skeleton, **allow removed** | the named painters + span builders in `render.rs` | `clippy::too_many_lines` (no allow on `render`), `module_budgets` ≤900 |

Counter-caveat honored: no over-fragmentation — H11 leaves 12 small non-edit arms in place rather
than scattering them across micro-modules; H14 keeps everything in `render.rs`; H9 creates exactly
one module for one axis of change.

---

## 8. Test strategy

### 8.1 Tests that move (the only test edits in the effort)

The 11 logical-line tests + the `fn buf` helper, `derive.rs` → `lines.rs` (§3.4), assertions
untouched. Nothing else moves: H11's 52 `commands.rs` tests all drive `run(Command…)` or the
builders and stay put; H14 moves zero tests.

### 8.2 Behavior-identical assertion, per item

- **H9:** pure-move + re-export; `cargo test` across all suites is the proof (the ~50 external
  call sites compile unchanged; the 11 moved tests pass unchanged in their new home).
- **H11:** the 52 `commands.rs` tests (buffer state, `nav::head`, `CommandResult`,
  dirty/version — including the Quit tests at their `app.rs`/`commands.rs` sites) + the 25 e2e
  journeys. These pin the epilogue semantics (desired_col reset on edits, NOT on Move), the Noop
  guards, EditKind coalescing, and Cut's clipboard splice.
- **H14:** the 83 `render.rs` tests + 25 `e2e.rs` journeys via `render_to_buffer` (full cell-grid
  clone), `render_capturing_cursor` (gates `place_cursor`), `row_string`/`row_has_highlight`/
  `row_has_underline` — ~98 per-cell symbol+style assertion sites, including the named goldens
  (`golden_default_scrollbar_styled`, `…_list_bullet_darkgray_dim`, `…_blockquote_glyph…`,
  `…_fold_marker_darkgray`, `…_wrap_guide_darkgray`) and the a11y/canvas/status/dropdown suites.
  **All unchanged — byte-identical grids are the acceptance bar.**

### 8.3 Gates (all must pass at merge; run per-item during the branch)

1. `cargo test` green across all suites (`wordcartel-core` lib + oracle, `wordcartel` lib).
2. `cargo build` and `cargo test --no-run` warning-free for `wordcartel` (catches the derive.rs
   unused-import case, §3.2).
3. `cargo clippy --workspace --all-targets` clean (workspace denies `clippy::all`;
   `too_many_lines` threshold 100). New allows: NONE except the retained, re-rationalized allow on
   `run` (§4.5). If a genuinely flat span builder lands marginally over 100 despite the shared
   helpers, it takes an item-local allow with a one-line reason — expected NOT to be needed
   (§5.5 size estimates).
4. `wordcartel/tests/module_budgets.rs` — `render.rs` ≤900 production lines (§5.7); `app.rs` and
   `timers.rs` budgets untouched by this effort.
5. No `cargo fmt`; house style by review.
6. PTY smoke suite `scripts/smoke/run.sh` — mandatory-run, advisory-pass; the pre-merge report
   quotes its one-line summary verbatim.

### 8.4 Command-surface invariant tests

Palette-completeness, every-option-has-a-command, and hint re-resolution suites run unchanged
(§6) — no new commands means no new rows for them to check, and a regression in them would mean
this effort violated its own N/A claim.

---

## 9. Risks & mitigations

| # | Risk | Item | Mitigation |
|---|---|---|---|
| 1 | Span-boundary drift from the unification (e.g. `text_fg_or_base` moved relative to the run comparison) changes golden grids | H14 | §5.5/§5.6 pin the exact placement; helpers are verbatim factorings of textually-identical code; the ~98 cell-assertion sites + goldens are the oracle |
| 2 | Paint-order regression (scrollbar before rows, cursor before status, overlays not last) | H14 | §5.3 fixes the call order in `render()`; `render_capturing_cursor` + overlay/scrollbar goldens catch violations |
| 3 | `tg` recomputed inside a painter → paint/cursor desync under measure/centered mode | H14 | passed as `&tg` (§5.3); no painter calls `text_geometry`; review checklist item |
| 4 | Relocating phase-6/7 reads into `gather_row_ctx` observes different state | H14 | all reads pure; no paint or mutation between the old and new read points (§5.4) |
| 5 | `row_dim` rebuilt as `compose(…).add_modifier(DIM)` instead of the 4-element compose | H14 | explicit in `ladder_style`'s contract (§5.2/§5.6); FocusDim goldens |
| 6 | Epilogue helper accidentally applied to a non-edit arm (Move/CycleRenderMode/… gaining a desired_col reset) | H11 | helper lives in `edit.rs`, private; the 12 non-edit arms are byte-untouched (§4.2); desired_col-sensitive nav tests |
| 7 | EditKind drift (collapsed InsertChar losing `Type`) breaks undo coalescing | H11 | verbatim body move (§4.3); history/undo tests in `commands.rs` |
| 8 | Cut's clipboard splice lost between apply and settle | H11 | helper deliberately excludes `apply` (§4.2); Cut/clipboard tests |
| 9 | Early-`Noop` guards turn into fallthroughs when extracted | H11 | every `edit.rs` fn returns `CommandResult` (§4.3); Noop-asserting tests |
| 10 | Borrow-check failure from `active()`/`active_mut()` overlap in extracted fns | H11 | bodies move verbatim including their clone/snapshot patterns (`buf_snap`); `settle_after_edit(&mut Editor)` called after reads drop (§4.2) |
| 11 | `line_render_for` visibility break (`pub use` of a `pub(crate)` item) | H9 | split re-export: `pub use` for the three `pub` fns, `pub(crate) use` for `line_render_for` (§3.3) |
| 12 | Bare-name calls in `rebuild_downstream` stop resolving | H9 | the re-export `use` itself brings the names into `derive`'s scope (§3.3) — no body edits |
| 13 | Unused `TextBuffer` import warning in `derive.rs` fails the warning-free gate | H9 | checked at implementation; removed if orphaned (§3.2) |
| 14 | `module_budgets` breach from helper scaffolding | H14 | arithmetic in §5.7 (≈775 of 900); the gate itself is the tripwire |
| 15 | Line anchors in this spec drift during implementation | all | implementers anchor on symbol names; cargo (not editor diagnostics) verifies changed code, per the CLAUDE.md tooling rule |

**Where the maps were insufficient (flagged, not invented):** none blocking. Two small items
verified directly for this spec rather than taken from the maps: (a) eng-health H9's `transform.rs`
consumer claim is a false positive (grep: only `extend_to_line_start`, its own fn); (b)
`line_layouts` is a `BTreeMap` (the H14 design leaves the existing collect+`sort_unstable`
verbatim regardless). The e2e journeys' exact coverage of Cut-with-clipboard and undo-coalescing
was not exhaustively enumerated — the plan's per-task test lists should name the specific tests
guarding risks 7–9 when tasks are written.

---

## History

- 2026-07-10 — drafted (Fable, design-author thread) for Codex spec review.
