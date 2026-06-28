# Wordcartel Effort 8 — Editor Essentials — Design

**Status:** design (brainstormed 2026-06-27)
**Roadmap:** Effort 8, exec #1, pre-1.0 (`docs/superpowers/plans/2026-06-22-wordcartel-coverage-ledger.md`).
**Goal:** add three small, universal editor commands that a basic editor is expected to
have and wordcartel currently lacks — **Select All**, **Go to line**, and a
**cursor-position (`Ln, Col`) status indicator** — surfaced by a 2026-06-27 feature audit.

All three register through the §10.4 name-keyed command registry, so they are
palette-reachable in **every** keymap preset; `cua`-default keybindings are added.
WordStar-preset bindings are out of scope here (Effort 9B).

---

## 1. Select All — `select_all`

**Behavior.** Set the primary selection to span the whole active buffer:
`Selection::range(0, buffer.len())` — anchor at byte 0, head at `buffer.len()`, so the
caret lands at the end and the selection runs forward. This **replaces** any existing
selection (single primary range; consistent with the current selection model). On an
empty buffer, `range(0, 0)` is an empty selection (no-op-safe). After applying, call
`nav::ensure_visible` (caret moved to end → scroll if needed).

**Keybinding.** `Ctrl+A` in the `cua` preset (free in `cua`; note the `wordstar` preset
already uses `Ctrl+A` for `move_left`, but WordStar bindings are deferred to Effort 9B).
Universal Select-All convention.

**Caret side-state (Codex).** Both this command and `goto_line` replace the selection
directly, so they must also reset caret bookkeeping the way the normal movement paths do:
clear `editor.active_mut().desired_col = None` (no stale vertical column) and
`editor.active_mut().sel_history.clear()` (the expand-selection ladder resets on motion).

**Notes.** Pure selection-state change; no buffer edit, no derive/relayout needed
beyond `ensure_visible`. Reuses `wordcartel_core::selection::Selection::range`. The active
buffer's selection is set via `editor.active_mut().document.selection = …`.

---

## 2. Go to line — `goto_line`

**Behavior.** `Ctrl+G` opens a minibuffer prompt `Go to line:`. On Enter:
1. Trim the input; parse as a **1-based** line number (`usize`).
2. Non-numeric or empty → no-op; set `editor.status = "not a line number"`; close the minibuffer.
3. Valid → it's a long-range jump, so **record the jump origin for jump-back** first
   (`let pre = nav::head(editor); marks::record_jump(editor.active_mut(), pre);` — same as
   `DocStart`/`DocEnd`/mark jumps, so `jump_back` returns here). Then **clamp** using the
   real total-line helper: `total = derive::total_logical_lines(buf); clamped =
   parsed.max(1).min(total); line_index = clamped - 1`. (Line 0 / beyond-EOF never errors —
   first/last line. Empty buffer is safe: `total_logical_lines("") == 1`, `line_to_byte(0) == 0`.)
4. Move the caret to **column 1** of the target line via the **fold-aware** caret placement
   the other jumps use (Codex): `let caret = buffer.line_to_byte(line_index);
   place_caret_visible(editor, caret, CaretPlace::UnfoldTo)` — so a target inside a folded
   (5g) body **unfolds** rather than landing on a hidden line.
5. Collapse the selection to the caret, **clear `desired_col`/`sel_history`** (per §1's caret
   side-state note), then `nav::ensure_visible`.

`line:col`, relative (`+N`/`-N`), and percentage (`NN%`) inputs are **out of scope**
(YAGNI; easy to add later).

**Keybinding.** `Ctrl+G` in the `cua` preset (verified free in `CUA`).

**Infra change — minibuffer `kind`.** The minibuffer submit path is currently
hardwired to *filter* (`app.rs` takes `minibuffer.text` and calls `dispatch_filter`).
Add a discriminant so submit routes correctly:
```rust
pub enum MinibufferKind { Filter, GotoLine }
// Minibuffer { prompt, text, cursor, kind }
```
`open_minibuffer` gains a `kind` argument (existing filter caller passes `Filter`); the
submit handler matches `kind` → `dispatch_filter` vs the goto-line action. This is the
only structural change; everything else reuses the existing minibuffer overlay (input,
cursor, Esc-cancel, XOR with other overlays).

---

## 3. Cursor-position indicator (`Ln, Col`)

**What it shows.** `Ln {line}, Col {col}` where:
- **`line`** = 1-based **logical** line: `buffer.byte_to_line(caret) + 1`.
- **`col`** = 1-based **source grapheme column**: the number of grapheme clusters from
  the start of the caret's logical line (`buffer.line_to_byte(line_index)`) up to the
  caret, plus 1. Uses the same grapheme-width/segmentation helper the layout already
  relies on (`unicode-segmentation`), counting **graphemes** (so a combining cluster or
  an accented `é` counts as one column).

**Locked semantics (decision A).** The column is the position in the **document source**,
NOT the on-screen visual column. Therefore it is **view-independent** (identical in
LivePreview, SourceHighlighted, and SourcePlain) and **wrap-independent** (counts within
the logical line, regardless of soft-wrap). Rationale: wordcartel renders the *active*
(caret) line as raw source even in live-preview, so source-col and visual-col already
coincide for the caret on its own line except under soft-wrap; choosing source-col makes
"same number regardless of view or window width" an explicit guarantee.

**Placement (decision B).** `Ln, Col` rides the **existing right-hand status segment**,
which today appears **only when `view_opts.word_count` is enabled**
(`word_count_segment`). When word-count is on, the right segment renders position **and**
count together, e.g.:
```
…path [LivePreview]                         Ln 12, Col 28 · 215 words · 1.3k chars
```
Format: `Ln {n}, Col {n} · {existing word-count text}`. When `word_count` is off, no
right segment (and no position) is shown — position display is gated on the same toggle.
A dedicated always-on position toggle is a possible future addition but is **out of
scope** here.

**Implementation.** Computed from the caret each frame; no editor state. Put the helper in
**`wordcartel-core`** (where `unicode-segmentation` is already a dependency — it is NOT a
direct dep of the `wordcartel` shell), as a pure fn
`caret_line_col(buffer, caret) -> (usize /*1-based line*/, usize /*1-based grapheme col*/)`,
and call it from `render.rs`. **Cost: O(line), not O(doc)** — the grapheme scan must run
over the slice `line_to_byte(line)..caret` (the caret's line only), never from byte 0.
`render.rs`'s right-segment assembly (the `word_count_segment` flush-right logic) prepends
the position to the existing count text.

---

## 4. Files touched

| File | Change |
|---|---|
| `wordcartel-core/src/…` | pure `caret_line_col(buffer, caret) -> (line, col)` helper (graphemes over `line_to_byte(line)..caret`; uses the core `unicode-segmentation` dep) |
| `wordcartel/src/registry.rs` | register `select_all`, `goto_line`; add `cua` binds `ctrl-a`→`select_all`, `ctrl-g`→`goto_line` |
| `wordcartel/src/commands.rs` | `select_all` handler (whole-buffer selection + clear `desired_col`/`sel_history` + `ensure_visible`); `goto_line` action (`record_jump` origin → parse → `total_logical_lines` clamp → `place_caret_visible(UnfoldTo)` → collapse selection + clear side-state + `ensure_visible`) |
| `wordcartel/src/minibuffer.rs` | add `MinibufferKind { Filter, GotoLine }` + `kind` field |
| `wordcartel/src/editor.rs` | `open_minibuffer(prompt, kind)` signature; init `kind` (existing filter caller passes `Filter`) |
| `wordcartel/src/app.rs` | minibuffer submit routes on `kind` (Filter → `dispatch_filter`; GotoLine → the goto action) |
| `wordcartel/src/render.rs` | right-segment prepends `Ln, Col` (via the core `caret_line_col`) to the word-count text |
| _tests_ | update the one test that constructs a `Minibuffer { … }` literal to include `kind` |

## 5. Testing

- **select_all:** whole non-empty buffer (range == 0..len, caret at end); empty buffer (empty selection, no panic); `desired_col`/`sel_history` cleared after.
- **goto_line:** valid mid-document line lands at that line's start; clamp-low (`0`/`1` → line 1); clamp-high (> total → last line); non-numeric → no-op + status; empty → no-op. Caret lands at column 1 (`line_to_byte`). **Jump-back:** after a goto, `jump_back` returns to the prior position (origin was recorded). **Fold:** a goto into a folded body unfolds to reveal the target (not hidden). `desired_col`/`sel_history` cleared.
- **caret_line_col:** ASCII; a line with a multibyte char (`é`) — column counts graphemes not bytes; a combining-grapheme cluster counts as one; caret at line start → Col 1; caret at end of line → Col = grapheme-count+1; **view-independence** — the same caret yields the same `(line, col)` under LivePreview and SourcePlain.
- **render:** with `word_count` on, the right segment contains `Ln ` and the count; with `word_count` off, neither appears. Tiny-terminal truncation still guarded.
- **minibuffer kind:** the existing filter-submit test still passes (passes `Filter`); a new goto-line submit test moves the caret.

## 6. Out of scope (explicitly deferred)

- `line:col` / relative (`+N`) / percentage go-to-line input.
- A dedicated always-on cursor-position toggle independent of word-count.
- Selection-size readout in the position segment (the word-count segment already counts the selection when one is active).
- WordStar-preset keybindings for these commands (Effort 9B).
- Tier-2/3 audit follow-ons (revert/reload, Tab→indent, line ops, help cheatsheet) — tracked on the roadmap, not this effort.
