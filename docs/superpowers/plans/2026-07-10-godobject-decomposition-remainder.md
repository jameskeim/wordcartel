# Implementation plan — god-object decomposition remainder (H9 + H11 + H14)

**Spec:** `docs/superpowers/specs/2026-07-10-godobject-decomposition-remainder-design.md` (Codex
GO-WITH-FIXES + a follow-up gate; the `RowCtx` dead-field findings are folded — `RowCtx` holds
**exactly the 12 fields `paint_rows`/`row_spans_*` read**; `has_block`, `block_hidden`, and
`diag_active` are gather-time locals, never struct fields).
**Maps (ground truth):** `.git/sdd/briefs/map-h9.md`, `map-h11.md`, `map-h14.md`.

## Goal

Finish the H1-class god-object decomposition as ONE behavior-identical refactor: leave
`derive::rebuild_downstream`, `commands::run`, and `render()` as thin dispatchers that delegate
into focused domain modules. Zero user-visible change; every existing test passes unchanged except
the ~11 logical-line tests that MOVE with their subject to `lines.rs`.

## Architecture

- **H9** (Task 1): new `wordcartel/src/lines.rs` owns the four pure logical-line helpers;
  `derive.rs` re-exports them so all ~50 call sites are untouched.
- **H11** (Task 2): new `wordcartel/src/commands/edit.rs` (a `commands` submodule — `commands.rs`
  + a `commands/` dir, no `mod.rs` conversion) owns the 8 buffer-edit primitives + the shared
  `settle_after_edit` epilogue; `commands::run`'s 8 edit arms become one-line delegations.
- **H14** (Tasks 3 + 4): `render()`'s body splits into `paint_status`/`place_cursor` (Task 3) and
  `gather_row_ctx`/`paint_rows`/`row_spans_segs`/`row_spans_placed` + the unification helpers
  `push_prefix_lead_in`/`ladder_style` (Task 4), all within `render.rs`. `render()` becomes a
  ≈90-line skeleton and sheds its `#[allow(clippy::too_many_lines)]`.

## Tech stack

Rust (workspace: `wordcartel-core` pure lib, `wordcartel` shell), ratatui 0.30, crossterm.
Edits are all in the `wordcartel` shell crate. No new dependencies.

## Global constraints (GATES — all must pass to merge; run per task)

- `cargo test` green across ALL suites (`wordcartel-core` lib + oracle, `wordcartel` lib).
- `cargo build` AND `cargo test --no-run` **warning-free** for touched crates (the H9 orphaned-
  import case and any dead field/fn are caught here).
- `cargo clippy --workspace --all-targets` **clean** — workspace denies `clippy::all`;
  `too_many_lines` threshold 100. A new `#[allow(...)]` requires an item-local one-line rationale;
  the ONLY retained allow is the re-rationalized one on `commands::run`. `render()`'s allow is
  REMOVED.
- `wordcartel/tests/module_budgets.rs` — `render.rs` production (pre-`mod tests`) ≤ 900 lines.
- **DO NOT run `cargo fmt`.** Hand-match the surrounding dense house style; em-dash `—` (never
  `--`) in prose comments; no reflow of untouched code.
- **Command-surface-contract conformance:** H11 is dispatcher-INTERNAL — it adds/removes/changes
  NO command; `Command`/`CommandResult`/`run`'s signature unchanged; the public builders
  `build_multi_replace`/`build_range_replace`/`scope_range_at` stay `pub` in `commands.rs` at their
  current paths. H9/H14 do not touch the command surface (N/A). The contract's invariant tests
  (palette-completeness, every-option-has-a-command, hint re-resolution) run unchanged.
- **Behavior-identical bar:** H14 render golden / cell-grid tests pass UNCHANGED — never weaken or
  edit a golden; only NEW behavior-preservation assertions may be added. H9's ~11 logical-line
  tests move verbatim (assertions untouched). H11 preserves per-arm `EditKind`, the early-`Noop`
  guards, and Cut's clipboard-splice ordering.
- **Commit trailers held** — do NOT commit until the human asks. The per-task "commit" steps below
  describe the intended unit boundary; execute the actual `git commit` only on explicit request,
  and when you do, end every message with the two project trailers verbatim.

**Tooling rule:** for compile/usage/signature questions on code you are editing, trust `cargo` +
`grep`, NOT an editor "unused"/"undefined" hint (rust-analyzer lags edits). Anchor on symbol
NAMES, not the line numbers in this plan (they drift as tasks land).

---

## Task 1 — H9: extract logical-line helpers into `lines.rs`

**Risk: trivial.** Verbatim move of four pure fns + a re-export; the ~50 external call sites and
`rebuild_downstream`'s bare calls all keep compiling.

### Files
- **Create** `wordcartel/src/lines.rs`.
- **Modify** `wordcartel/src/lib.rs` (register `pub mod lines;`).
- **Modify** `wordcartel/src/derive.rs` (delete the four fns; add re-export; move 11 tests out).

### Interfaces
Produced in `lines.rs` (signatures identical to today's `derive.rs`):
```rust
pub(crate) fn line_render_for(mode: crate::editor::RenderMode, is_active_line: bool)
    -> wordcartel_core::style::LineRender
pub fn total_logical_lines(buf: &TextBuffer) -> usize
pub fn line_start(buf: &TextBuffer, line: usize) -> usize
pub fn line_text(buf: &TextBuffer, line: usize) -> String
```
Consumed unchanged (via re-export) by `nav.rs`, `render.rs`, `commands.rs`, `prompts.rs`, and
`derive::rebuild_downstream`.

### Steps

**1.1 — Create `lines.rs` with the four fns moved verbatim.**
Create `wordcartel/src/lines.rs`. Its only `use` is `use wordcartel_core::buffer::TextBuffer;`
(the `RenderMode` and `LineRender` names stay path-qualified in `line_render_for`, exactly as
today). Move — cut, do not copy — the four functions **with their full doc comments** from
`derive.rs`: `line_render_for` (currently ~:25, including its `use crate::editor::RenderMode::*;`
and `use wordcartel_core::style::LineRender::*;` inner `use`s), `total_logical_lines` (~:91),
`line_start` (~:104), `line_text` (~:116). Prepend a module doc comment:
```rust
//! Pure logical-line helpers over `TextBuffer`: line count, line-start byte offset,
//! line text (trailing '\n' stripped), and the render-mode → `LineRender` mapping.
//! No dependence on the derive pipeline (`rebuild`/`LayoutKey`/caches).

use wordcartel_core::buffer::TextBuffer;
```

**1.2 — Register the module.**
In `wordcartel/src/lib.rs`, add `pub mod lines;` immediately after `pub mod derive;` (lib.rs:5).

**1.3 — Add the re-export in `derive.rs`.**
In `derive.rs`, immediately after the existing `use` block (after `use wordcartel_core::layout;`,
:4), add:
```rust
pub use crate::lines::{line_start, line_text, total_logical_lines};
pub(crate) use crate::lines::line_render_for;
```
This makes the bare-name calls inside `rebuild_downstream` (`total_logical_lines(buf)`,
`line_text(buf, l)`, `line_start(buf, l)`, `line_render_for(b_mode, …)`) resolve unchanged — do
NOT edit `rebuild_downstream`'s body.

**1.4 — Handle the now-possibly-orphaned `TextBuffer` import in `derive.rs`.**
`derive.rs` keeps `use wordcartel_core::buffer::TextBuffer;` (:3) ONLY if something outside the
moved fns still names `TextBuffer`. Determine by build, not by eye:
```
cargo build -p wordcartel 2>&1 | grep -n "unused import.*TextBuffer" || echo "TextBuffer still used"
```
If the warning fires, delete that one `use` line from `derive.rs`. (Same for `RenderMode`/
`LineRender` — but those were only referenced inside the moved `line_render_for`, so no top-level
`derive.rs` import existed for them; nothing to remove.)

**1.5 — Move the 11 logical-line tests to `lines.rs`.**
From `derive.rs`'s `mod tests`, cut the "Logical-line edge-case helpers" block: the local
`fn buf(s: &str) -> TextBuffer { TextBuffer::from_str(s) }` helper plus exactly these 11 tests —
`total_lines_empty_is_one`, `total_lines_no_newline`, `total_lines_trailing_newline_is_two`,
`total_lines_lone_newline`, `total_lines_two_lines_no_trailing_newline`, `line_start_positions`,
`line_text_strips_newline`, `line_text_empty_buffer`, `line_text_no_trailing_newline`,
`line_text_lone_newline`, `line_text_multibyte`. Paste them into a new test module at the bottom of
`lines.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn buf(s: &str) -> TextBuffer {
        TextBuffer::from_str(s)
    }

    // …the 11 tests, verbatim…
}
```
`use super::*;` supplies the four fns; `TextBuffer` comes from `lines.rs`'s own top-level `use`.
LEAVE `unicode_line_breaks_do_not_split_logical_lines` in `derive.rs` (it drives a full
`Editor`/rebuild and calls `crate::derive::total_logical_lines`, still valid via the re-export) and
leave `line_render_for`'s indirect coverage (`active_line_renders_raw`,
`caret_on_phantom_line_conceals_last_content_line`) in `derive.rs` untouched.

**1.6 — Verify (expect PASS; no RED step — this is a pure move, the moved tests are the oracle).**
```
cargo test -p wordcartel --lib lines::        # the 11 moved tests run in their new home
cargo test -p wordcartel --lib                # derive/nav/render/commands all still green
cargo build -p wordcartel 2>&1 | grep -E "warning|error" || echo "warning-free"
cargo clippy --workspace --all-targets 2>&1 | grep -E "warning|error" || echo "clippy clean"
```
Expected: `lines::tests` shows 11 passed; full lib green; no warnings; clippy clean.

**1.7 — Commit (on request):** `refactor(lines): H9 — lift logical-line helpers out of derive`.

---

## Task 2 — H11: decompose `commands::run`

**Risk: low.** Verbatim edit-body moves guarded by buffer-STATE tests. No command added/changed.

### Files
- **Create** `wordcartel/src/commands/edit.rs` (holds the 8 edit primitives AND `settle_after_edit`;
  `commands.rs` has no submodule today and `run` is a free fn, so the helper lives here, not in
  `commands.rs`).
- **Modify** `wordcartel/src/commands.rs` (add `mod edit;`; replace 8 arm bodies with delegations;
  prune now-orphaned top-level imports; re-rationalize `run`'s `#[allow]`; update the module doc
  comment).

### Interfaces
Produced in `commands::edit`:
```rust
fn settle_after_edit(editor: &mut Editor) -> CommandResult          // module-private
pub(super) fn insert_char(editor: &mut Editor, c: char, clock: &dyn Clock) -> CommandResult
pub(super) fn insert_newline(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
pub(super) fn backspace(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
pub(super) fn delete_forward(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
pub(super) fn cut(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
pub(super) fn delete_word(editor: &mut Editor, back: bool, clock: &dyn Clock) -> CommandResult
pub(super) fn delete_line(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
pub(super) fn delete_to_line_end(editor: &mut Editor, clock: &dyn Clock) -> CommandResult
```
Consumed by `commands::run` (delegation arms). Unchanged public builders in `commands.rs`:
`build_multi_replace`, `build_range_replace`, `scope_range_at` (stay `pub`); `replace_changeset`,
`scope_range`, `set_selection_range` (stay private).

### Coverage proof (the specific existing tests that guard the fragile paths)
These live in `commands.rs mod tests` (and `app.rs`) and MUST stay green unchanged — they pin the
exact semantics a verbatim move must preserve:
- **Cut clipboard-splice + register:** `select_right_twice_then_cut_removes_selection`
  (asserts buffer `"cd\n"`, caret 0, `register.get() == Some("ab")` after Cut) and
  `cut_on_empty_selection_is_noop` (Cut on point cursor → `Noop`, buffer unchanged). The
  `clipboard_sync_request` set-after-`apply` ordering is behavior-invariant (the sync read happens
  from `editor.register` after `apply` regardless of position within the arm); `Copy`'s analogue
  `copy_sets_register_and_sync_request` (app.rs) pins that `clipboard_sync_request` is populated
  from the register.
- **EditKind `Type` vs `Other` (undo coalescing):** `typing_coalesces_into_one_undo` — types
  "hi" at one timestamp and asserts a single undo restores `"\n"` (proves collapsed `InsertChar`
  keeps `EditKind::Type`); `delete_word_back_is_one_undo_step` (proves the delete arms' `Other`).
- **Early-`Noop` guards:** `delete_forward_at_eof_is_noop`, `backspace_at_start_is_noop`,
  `cut_on_empty_selection_is_noop`, `delete_to_line_end_at_eol_is_noop`,
  `select_all_empty_buffer_is_noop_safe` (SelectAll stays in `run`, unaffected), plus
  `delete_line_*` bodies (`delete_line_removes_whole_line_including_newline`,
  `delete_line_last_line_without_trailing_newline_vanishes`,
  `delete_line_on_empty_trailing_line_removes_preceding_newline`,
  `delete_line_single_line_empties_buffer`) and `delete_to_line_end_deletes_to_eol_keeps_newline`.
No new tests are required for H11; a NEW assertion may be added only if a reviewer wants an explicit
"Cut still sets `clipboard_sync_request`" check — optional, additive.

### Steps

**2.1 — Create `commands/edit.rs` skeleton with `settle_after_edit`.**
Create `wordcartel/src/commands/edit.rs`:
```rust
//! Buffer-edit primitives behind `commands::run`. Each returns `CommandResult`
//! (the arms have early Noop guards), ends non-Noop paths through
//! `settle_after_edit`, and preserves the exact `EditKind` of the original arm.

use crate::derive;
use crate::nav;
use crate::editor::Editor;
use super::{replace_changeset, CommandResult};
use wordcartel_core::block_tree::Edit;
use wordcartel_core::change::ChangeSet;
use wordcartel_core::history::{Clock, EditKind, Transaction};
use wordcartel_core::register;
use wordcartel_core::selection::Selection;

/// Post-edit epilogue shared by every buffer-edit primitive: re-derive, re-scroll,
/// re-anchor vertical motion (desired_col = None). Edit paths ONLY — Move/CycleRenderMode
/// keep rebuild+ensure_visible WITHOUT the desired_col reset; Undo/Redo/ShrinkSelection
/// insert a caret-snap first; SelectAll doesn't rebuild — those arms keep their own tails
/// in `run`.
fn settle_after_edit(editor: &mut Editor) -> CommandResult {
    derive::rebuild(editor);
    nav::ensure_visible(editor);
    editor.active_mut().desired_col = None;
    CommandResult::Handled
}
```
Note `super::{replace_changeset, CommandResult}` — `replace_changeset` stays private in
`commands.rs` and is reachable from the child module via `super::`. `ChangeSet` is imported for the
`ChangeSet::insert`/`ChangeSet::delete` calls in the moved bodies.

**2.2 — Register the submodule.**
In `commands.rs`, after the top `use` block (after `use wordcartel_core::selection::Selection;`,
:22), add:
```rust
mod edit;
```

**2.3 — Move the 8 edit-arm bodies verbatim, swapping only the epilogue.**
For each of the 8 arms below, cut the body out of `run`'s `match` and paste it into the named
`edit.rs` fn, changing ONLY the trailing epilogue lines
`derive::rebuild(editor); nav::ensure_visible(editor); editor.active_mut().desired_col = None;
CommandResult::Handled` → `settle_after_edit(editor)` (a `return …;` epilogue in a sel-branch
becomes `return settle_after_edit(editor);`). Keep every comment, every `EditKind`, every early
`return CommandResult::Noop`, and every clone/snapshot (`buf_snap`) unchanged.

- `insert_char(editor, c, clock)` — from `Command::InsertChar(c)` (both the sel-branch
  `EditKind::Other` and the collapsed `EditKind::Type` path; the `let c = …` is now the `c`
  parameter).
- `insert_newline(editor, clock)` — from `Command::InsertNewline` (both branches `EditKind::Other`;
  keep the coalescing-break comment).
- `backspace(editor, clock)` — from `Command::Backspace` (sel-branch + `head == 0` Noop + the
  `move_left` prev-stop path).
- `delete_forward(editor, clock)` — from `Command::DeleteForward` (sel-branch + `next == head`
  Noop).
- `cut(editor, clock)` — from `Command::Cut`. **Preserve the splice ordering exactly:**
  `editor.apply(txn, edit, EditKind::Other, clock);` then
  `if let Some(text) = editor.register.get().map(str::to_owned) { editor.clipboard_sync_request =
  Some(text); }` then `settle_after_edit(editor)`. Keep the empty-selection `Noop` guard and the
  `buf_snap` clone.
- `delete_word(editor, back, clock)` — from `Command::DeleteWord { back }` (the `back` bool is now
  the parameter; `from == to` Noop; `EditKind::Other`).
- `delete_line(editor, clock)` — from `Command::DeleteLine` (the `len == 0` Noop, the
  `from == to` Noop, the phantom-line / no-trailing-newline range logic verbatim; it calls
  `derive::total_logical_lines` — valid via the H9 re-export).
- `delete_to_line_end(editor, clock)` — from `Command::DeleteToLineEnd` (the `head >= to` Noop;
  calls `derive::total_logical_lines`).

**2.4 — Replace the 8 arms in `run` with delegations.**
In `run`'s `match`, the 8 arms become exactly:
```rust
Command::InsertChar(c)       => edit::insert_char(editor, c, clock),
Command::InsertNewline       => edit::insert_newline(editor, clock),
Command::Backspace           => edit::backspace(editor, clock),
Command::DeleteForward       => edit::delete_forward(editor, clock),
Command::Cut                 => edit::cut(editor, clock),
Command::DeleteWord { back } => edit::delete_word(editor, back, clock),
Command::DeleteLine          => edit::delete_line(editor, clock),
Command::DeleteToLineEnd     => edit::delete_to_line_end(editor, clock),
```
The other 12 arms (`Move`, `Copy`, `Paste`, `Undo`, `Redo`, `CycleRenderMode`, `Save`, `Quit`,
`SelectScope`, `ExpandSelection`, `ShrinkSelection`, `SelectAll`) are byte-untouched.

**2.4a — Prune `commands.rs`'s now-orphaned top-level imports.**
Moving the 8 edit arms out removes the only bare uses of `Edit`, `ChangeSet`, `EditKind`, and
`Transaction` in production `run` — verified against real source: every bare `Edit { … }`
(commands.rs:221/235/252/266/284/304/320/342/431/559/596/622), every bare `ChangeSet::insert|delete`
(:233/264/283/303/319/341/558/595/621), every `EditKind::` (:223/237/254/270/286/306/322/345/433/562/598/624),
and every `Transaction::new` (:222/236/253/269/285/305/321/344/432/560/597/623) sits inside a moving
edit arm. The surviving code does NOT need them: `build_multi_replace`/`build_range_replace` use the
full path `wordcartel_core::block_tree::Edit` (:152/162) and `replace_changeset`/`build_multi_replace`
carry their OWN inner `use wordcartel_core::change::{ChangeSet, Op, Tendril};` (:111/135); the one test
that builds a transaction (`mod tests`, the `apply` test) has its own local
`use wordcartel_core::history::{EditKind, Transaction};` — so nothing resolves these four through the
top-level import once the arms leave. Edit the top-level `use` block (commands.rs:18–20):
- DELETE `use wordcartel_core::block_tree::Edit;` (:18).
- DELETE `use wordcartel_core::change::ChangeSet;` (:19).
- CHANGE `use wordcartel_core::history::{Clock, EditKind, Transaction};` → `use wordcartel_core::history::Clock;`
  (`Clock` stays — it is in `run`'s signature).
KEEP `use wordcartel_core::register;` (the `Copy` arm calls `register::copy`, :413) and
`use wordcartel_core::selection::Selection;` (used by `Move`/`Undo`/`Redo`/`SelectAll`/`set_selection_range`,
:204/393/…). Do NOT touch `use crate::{derive, editor::{Editor, RenderMode}, file, nav};` or
`use crate::registry::{place_caret_visible, CaretPlace};` — all still used by surviving arms. The
`cargo build` warning-free gate (step 2.7, which does NOT compile the test module) is the arbiter: an
over-prune yields "cannot find type", an under-prune yields "unused import" — both fail it, so re-run
2.7 after this edit.

**2.5 — Re-rationalize `run`'s `#[allow]`.**
`run` stays over 100 lines (12 non-edit arms keep their bodies). Update the allow comment (:210):
```rust
#[allow(clippy::too_many_lines)] // exhaustive flat Command dispatch — edit arms delegate to
                                 // commands::edit; remaining arms are small non-edit state ops (H11)
```

**2.6 — Update the module doc comment.**
`commands.rs:1–11` narrates "Every edit command: 1…6". Update it to note that steps 3–6 (apply →
rebuild → ensure_visible → desired_col reset) now live in `commands::edit` behind
`settle_after_edit`. Doc-accuracy only.

**2.7 — Verify (expect PASS — verbatim move; the coverage-proof tests are the oracle).**
```
cargo test -p wordcartel --lib commands::             # 52 command tests, incl. the coverage-proof set
cargo test -p wordcartel --lib                         # full lib green (e2e journeys included)
cargo build -p wordcartel 2>&1 | grep -E "warning|error" || echo "warning-free"
cargo clippy --workspace --all-targets 2>&1 | grep -E "warning|error" || echo "clippy clean"
```
Expected: all command tests pass (specifically `typing_coalesces_into_one_undo`,
`select_right_twice_then_cut_removes_selection`, the five `*_is_noop`, the `delete_line_*`); no
warnings (catches an accidentally-unused import in `edit.rs` or a dead `pub(super)` fn); clippy
clean.

**2.8 — Commit (on request):** `refactor(commands): H11 — lift edit arms into commands::edit`.

---

## Task 3 — H14a: extract `paint_status` + `place_cursor`

**Risk: medium.** Two self-contained trailing phases; lower risk than the row loop. Split from
Task 4 so a reviewer can accept the status/cursor extraction independently.

### Files
- **Modify** `wordcartel/src/render.rs` (extract two fns; call them from `render()`).

### Interfaces
Produced (private, above the `#[cfg(test)] mod tests` boundary):
```rust
fn paint_status(frame: &mut Frame, editor: &Editor, area: Rect, status_row: u16, cs: &ChromeStyles)
fn place_cursor(frame: &mut Frame, editor: &Editor, area: Rect, edit_top: u16,
                edit_height: u16, status_row: u16, tg: &nav::TextGeometry)
```
Consumed by `render()`. Both read `editor` immutably; derive `w = area.width` internally.

### Steps

**3.1 — Extract `paint_status`.** Byte-identical move; do NOT alter logic. Cut the STATUS `{ … }`
scope (render.rs :635–699) out of `render()` and paste its body into the fn below. The block's
inputs (`w`, `status_row`, `cs`, `area`) are now the params/`let w = area.width;`; no other locals
cross in. This is the complete literal body to produce (pulled from current source — compare against
render.rs before/after to confirm nothing but the wrapping changed):
```rust
/// Phase 11 — the bottom status row: search bar / minibuffer / prompt / normal /
/// calm-hidden selection, right-flush Ln/Col/words composition, full-row set_style
/// THEN the Paragraph. Reads editor immutably.
fn paint_status(frame: &mut Frame, editor: &Editor, area: Rect, status_row: u16, cs: &ChromeStyles) {
    let w = area.width;
    // When the search overlay is active, render the search bar.
    // When a modal prompt is active, render its message instead of the normal
    // status text, using a distinct style so it stands out.
    // When the minibuffer is open, render <prompt><text> on the status row.
    let (status_text, status_style) = if let Some(ref s) = editor.search {
        (
            crate::render_status::format_search_bar(s),
            cs.ov_accent,
        )
    } else if let Some(ref mb) = editor.minibuffer {
        (
            format!("{}{}", mb.prompt, mb.text),
            cs.ov_accent,
        )
    } else if let Some(ref prompt) = editor.prompt {
        (
            prompt.message.clone(),
            cs.ov_accent,
        )
    } else {
        // Normal state. Under zen/Auto idle with no message, the reserved row renders
        // as calm canvas (base bg); visible reveal via On / dwell / message force.
        if crate::chrome::status_line_visible(editor) {
            (crate::render_status::status_left_text(editor), cs.menu_closed) // visible: [Chrome] panel bg
        } else {
            // Calm canvas: the same bg-only fill the edit band uses — NOT chrome.
            let mut calm = compose::base_canvas(&editor.theme, editor.depth);
            calm.fg = None;
            (String::new(), calm)
        }
    };

    // Compose the status line.
    // When in the normal branch (no prompt/minibuffer/search) and word_count is on,
    // flush the count segment to the right and truncate the left (path/mode) to fit.
    // When the status row is calm-hidden (Auto idle, no message), suppress the word-count
    // segment so Ln/Col · words does not paint over the calm canvas row.
    let has_overlay = editor.search.is_some() || editor.minibuffer.is_some() || editor.prompt.is_some() || editor.diag.is_some() || editor.outline.is_some();
    let status_hidden = !has_overlay && !crate::chrome::status_line_visible(editor);
    let composed = if !has_overlay && !status_hidden {
        if let Some(wc) = crate::render_status::word_count_segment(editor) {
            let caret = crate::nav::head(editor);
            let (l, c) = editor.active().document.buffer.caret_line_col(caret);
            let right = format!("Ln {l}, Col {c} · {wc}");
            let reserve = right.chars().count() + 1;
            let left: String = status_text.chars().take((w as usize).saturating_sub(reserve)).collect();
            let pad = (w as usize).saturating_sub(left.chars().count() + right.chars().count());
            format!("{left}{}{right}", " ".repeat(pad))
        } else {
            status_text.chars().take(w as usize).collect()
        }
    } else {
        status_text.chars().take(w as usize).collect()
    };
    // Truncate the composed string to the terminal width (guard for very narrow terminals).
    let truncated: String = composed.chars().take(w as usize).collect();
    let status_line = Line::from(Span::styled(truncated, status_style));
    let status_area = Rect::new(area.x, status_row, w, 1);
    // Fill the WHOLE row with the state's chrome style first — the Paragraph
    // styles only the text span, and a partial bar next to the full-width menu
    // bar was the reported-mismatch class (Fable whole-branch I-2).
    frame.buffer_mut().set_style(status_area, status_style);
    frame.render_widget(Paragraph::new(status_line), status_area);
}
```

**3.2 — Extract `place_cursor`.** Byte-identical move; do NOT alter logic. Cut the CURSOR block
(render.rs :701–734 — the three `if let … editor.search / editor.minibuffer / nav::screen_pos`
arms) and paste its body below. Complete literal body:
```rust
/// Phase 12 — the hardware cursor: search-field / minibuffer / normal-caret arms,
/// char-count column math, D2 clamp of the normal caret col to tg.text_width.
fn place_cursor(frame: &mut Frame, editor: &Editor, area: Rect, edit_top: u16,
                edit_height: u16, status_row: u16, tg: &nav::TextGeometry) {
    let w = area.width;
    if let Some(ref s) = editor.search {
        // Search bar is open: place caret on the status row at the focused field's caret.
        // Use char counts (not byte offsets) for correct placement with multibyte text.
        let prefix_cols = match s.field {
            crate::search_overlay::Field::Needle => "Find: ".chars().count(),
            crate::search_overlay::Field::Template =>
                format!("Find: {}  Replace: ", s.needle).chars().count(),
        };
        let caret_cols = s.focused_field()[..s.cursor].chars().count();
        let x_offset = (prefix_cols + caret_cols) as u16;
        if x_offset < w {
            frame.set_cursor_position(Position { x: area.x + x_offset, y: status_row });
        }
    } else if let Some(ref mb) = editor.minibuffer {
        // Minibuffer is open: place caret on the status row at prompt.len() + cursor.
        // cursor is a byte offset; for display we want the char count so the terminal
        // column is correct even for multi-byte prompts/text (small strings, safe).
        let prompt_cols = mb.prompt.chars().count() as u16;
        let text_cols = mb.text[..mb.cursor].chars().count() as u16;
        let caret_col = prompt_cols + text_cols;
        if caret_col < w {
            frame.set_cursor_position(Position { x: area.x + caret_col, y: status_row });
        }
    } else if let Some((col, row)) = nav::screen_pos(editor) {
        // Guard rows; clamp cols — a caret on/after hung trailing whitespace sits
        // logically past the text rect and pins at the edge (spec D2 clamp).
        if row < edit_height && tg.text_width > 0 {
            let col = col.min((tg.text_width as usize).saturating_sub(1) as u16);
            frame.set_cursor_position(Position { x: area.x + tg.text_left + col, y: edit_top + row });
        }
    }
}
```

**3.3 — Wire the calls into `render()`.**
Replace the two moved blocks in `render()` (in place, preserving order — status BEFORE cursor,
both AFTER the scrollbar and BEFORE `render_overlays::paint`) with:
```rust
paint_status(frame, editor, area, status_row, &cs);
place_cursor(frame, editor, area, edit_top, edit_height, status_row, &tg);
```
Do NOT touch the `tg = nav::text_geometry(editor)` single call, the canvas fill, wrap-guide, row
loop, `ChromeStyles::build`, scrollbar, or `render_overlays::paint(frame, editor, &cs)`.

**3.4 — Verify (expect PASS — golden grids UNCHANGED).**
```
cargo test -p wordcartel --lib render::                # 83 render tests incl. status/cursor goldens
cargo test -p wordcartel --lib e2e::                   # 25 e2e journeys
cargo test -p wordcartel --lib
cargo build -p wordcartel 2>&1 | grep -E "warning|error" || echo "warning-free"
cargo clippy --workspace --all-targets 2>&1 | grep -E "warning|error" || echo "clippy clean"
```
Expected: all render + e2e tests green, byte-identical cell grids (specifically the status suites
and `render_capturing_cursor`-gated cursor tests); no `too_many_arguments` clippy (both fns ≤7
args — `place_cursor` is exactly 7); warning-free.

**3.5 — Commit (on request):** `refactor(render): H14a — extract paint_status + place_cursor`.

---

## Task 4 — H14b: extract `gather_row_ctx` + `paint_rows` + segs/placed unification

**Risk: high (the golden-grid gate).** The row loop, its snapshot context (exactly the 12 fields
the paint path reads), both span
builders, and the two unification helpers. `render()` drops its `#[allow]`.

### Files
- **Modify** `wordcartel/src/render.rs`.

### Interfaces
Produced (private, above `#[cfg(test)] mod tests`):
```rust
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
}  // EXACTLY the 12 fields paint_rows/row_spans_* read. Reads: paint_rows → scroll,
   // focus_region, sorted_lines, use_placed; row_spans_segs → plain_source; row_spans_placed →
   // diag_all, hl_current, hl_window, marked_block, has_sel, sel_from, sel_to, plain_source.
   // has_block/block_hidden/diag_active are gather-time locals feeding use_placed/diag_all —
   // NOT struct fields (a written-but-unread field trips the warning-free gate).

fn gather_row_ctx(editor: &Editor) -> RowCtx<'_>
fn paint_rows(frame: &mut Frame, editor: &Editor, area: Rect,
              edit_top: u16, edit_height: u16, tg: &nav::TextGeometry)
fn row_spans_segs(editor: &Editor, ctx: &RowCtx, vr: &VisualRow, map: &ColMap,
                  row_dim: bool) -> Vec<Span<'static>>
fn row_spans_placed(editor: &Editor, ctx: &RowCtx, l: usize, row_index: usize,
                    vr: &VisualRow, map: &ColMap, row_dim: bool) -> Vec<Span<'static>>
fn push_prefix_lead_in(spans: &mut Vec<Span<'static>>,
                       theme: &wordcartel_core::theme::Theme,
                       depth: wordcartel_core::theme::Depth,
                       vr: &VisualRow, map: &ColMap, row_dim: bool)
fn ladder_style(theme: &wordcartel_core::theme::Theme, depth: wordcartel_core::theme::Depth,
                role: wordcartel_core::style::BlockRole, inline: wordcartel_core::style::Style,
                row_dim: bool, plain_source: bool) -> RStyle
```
(`VisualRow`/`ColMap`/`Placed` come from `wordcartel_core::layout`; add `use` aliases if the file
doesn't already name them — verify with grep before adding, to avoid an unused/duplicate import.)

### Steps

**4.1 — Add `ladder_style` (the shared 4-arm style ladder), keyed on the `Style` value.**
This is the exact ladder duplicated at render.rs :434–446 (segs) and :515–527 (placed):
```rust
/// The per-glyph style ladder shared by both row-span builders, keyed on the inline
/// `Style` value (seg.style / p.style). The dim non-plain arm is the distinct
/// 4-element compose [Text, role, style, FocusDim] (NOT compose(...).add_modifier(DIM)) —
/// preserves heading bold / comment italic on dim rows (§13.2 FIX-1).
fn ladder_style(theme: &wordcartel_core::theme::Theme, depth: wordcartel_core::theme::Depth,
                role: wordcartel_core::style::BlockRole, inline: wordcartel_core::style::Style,
                row_dim: bool, plain_source: bool) -> RStyle {
    if row_dim {
        if plain_source {
            compose::compose(theme, depth, &[SE::Text, SE::FocusDim])
        } else {
            compose::compose(theme, depth, &[SE::Text, role_element(role), style_element(inline), SE::FocusDim])
        }
    } else if plain_source {
        compose::compose(theme, depth, &[SE::Text])
    } else {
        compose::compose(theme, depth, &[SE::Text, role_element(role), style_element(inline)])
    }
}
```

**4.2 — Add `push_prefix_lead_in` (the shared prefix lead-in).**
This is the fragment duplicated at :404–432 (segs) and :477–503 (placed), textually identical
except the target vec name:
```rust
/// Prefix lead-in shared by both row-span builders. Row 0 of a prefixed line paints the
/// real glyph — a heading inverted-numeral box (REVERSED glyph + NORMAL space) when
/// heading_level_glyph is on, else the dim single-span glyph; continuation rows push a
/// prefix_width blank spacer so text stays aligned with the prefix-offset cursor columns.
fn push_prefix_lead_in(spans: &mut Vec<Span<'static>>,
                       theme: &wordcartel_core::theme::Theme,
                       depth: wordcartel_core::theme::Depth,
                       vr: &VisualRow, map: &ColMap, row_dim: bool) {
    if let Some(ref glyph) = vr.prefix_glyph {
        let pe = prefix_element(vr.role);
        let heading_n = if theme.heading_level_glyph {
            if let wordcartel_core::style::BlockRole::Heading(n) = vr.role { Some(n) } else { None }
        } else { None };
        if let Some(n) = heading_n {
            let g = HEADING_GLYPHS[(n.clamp(1, 6) - 1) as usize];
            let base = if row_dim {
                compose::compose(theme, depth, &[pe, SE::FocusDim])
            } else {
                compose::compose(theme, depth, &[pe])
            };
            spans.push(Span::styled(g.to_string(), base.add_modifier(Modifier::REVERSED)));
            spans.push(Span::styled(" ".to_string(), base));
        } else {
            let gstyle = if row_dim {
                compose::compose(theme, depth, &[pe, SE::FocusDim])
            } else {
                compose::compose(theme, depth, &[pe]).add_modifier(Modifier::DIM)
            };
            spans.push(Span::styled(glyph.clone(), gstyle));
        }
    } else if map.prefix_width > 0 {
        spans.push(Span::raw(" ".repeat(map.prefix_width)));
    }
}
```
Confirm this matches BOTH source copies byte-for-byte (only `segs_spans`/`hl_spans` differed);
if any token differs between the two copies, STOP — the unification premise is violated, flag it.

**4.3 — Add `row_spans_segs`.**
```rust
/// Segs fast path (true no-op rows, no per-glyph styling) — the builder for
/// !use_placed rows.
fn row_spans_segs(editor: &Editor, ctx: &RowCtx, vr: &VisualRow, map: &ColMap,
                  row_dim: bool) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    push_prefix_lead_in(&mut spans, &editor.theme, editor.depth, vr, map, row_dim);
    for seg in &vr.segs {
        let style = ladder_style(&editor.theme, editor.depth, vr.role, seg.style, row_dim, ctx.plain_source);
        let style = text_fg_or_base(style, &editor.theme, editor.depth);
        spans.push(Span::styled(seg.text.clone(), style));
    }
    spans
}
```
(`ctx` is unused beyond `plain_source` here — keep the `ctx` param for a uniform builder signature;
if clippy flags `unused_variables`/`ctx`, read only `ctx.plain_source` and drop the param instead.
Prefer passing `ctx` and reading `ctx.plain_source` so both builders share a shape — verify no
clippy `unused` fires; if it does, inline `ctx.plain_source` at the call and drop `ctx`.)

**4.4 — Add `row_spans_placed`.**
Move the placed-path body (:457–587) verbatim into this fn, changing ONLY: (a) `hl_spans` is the
returned `spans` vec, (b) the inline prefix lead-in block (:477–503) is replaced by
`push_prefix_lead_in(&mut spans, &editor.theme, editor.depth, vr, map, row_dim);`, (c) the ladder
at :515–527 is replaced by
`let mut style = ladder_style(&editor.theme, editor.depth, vr.role, p.style, row_dim, ctx.plain_source);`.
Everything else stays byte-identical and in order:
```rust
fn row_spans_placed(editor: &Editor, ctx: &RowCtx, l: usize, row_index: usize,
                    vr: &VisualRow, map: &ColMap, row_dim: bool) -> Vec<Span<'static>> {
    let buf = &editor.active().document.buffer;
    let line_off = derive::line_start(buf, l);
    let mut spans: Vec<Span<'static>> = Vec::new();
    let lo = line_off + vr.src_span.start;
    let hi = line_off + vr.src_span.end;
    let hi_idx = ctx.diag_all.partition_point(|d| d.range.start < hi);
    let diag_window: Vec<&wordcartel_core::diagnostics::Diagnostic> =
        ctx.diag_all[..hi_idx].iter().filter(|d| d.range.end > lo).collect();

    push_prefix_lead_in(&mut spans, &editor.theme, editor.depth, vr, map, row_dim);

    let mut run = String::new();
    let mut run_style: Option<RStyle> = None;
    for p in map.placed.iter().filter(|p| p.row == row_index) {
        let g_from = line_off + p.src.start;
        let g_to = line_off + p.src.end;
        let is_current = ctx.hl_current.is_some_and(|m| overlaps(g_from, g_to, m.start, m.end));
        let is_match = !is_current && ctx.hl_window.iter().any(|m| overlaps(g_from, g_to, m.start, m.end));

        let mut style = ladder_style(&editor.theme, editor.depth, vr.role, p.style, row_dim, ctx.plain_source);

        if let Some(b) = ctx.marked_block {
            if !b.hidden && overlaps(g_from, g_to, b.start, b.end) {
                let mb_face = editor.theme.face(SE::MarkedBlock);
                style = style.patch(crate::compose::face_to_ratatui(&mb_face, editor.depth));
            }
        }
        let is_selected = ctx.has_sel && overlaps(g_from, g_to, ctx.sel_from, ctx.sel_to);
        if is_selected {
            let sel_face = editor.theme.face(SE::Selection);
            style = style.patch(crate::compose::face_to_ratatui(&sel_face, editor.depth));
        }
        if is_current {
            let search_face = editor.theme.face(SE::SearchCurrent);
            let ss = crate::compose::face_to_ratatui(&search_face, editor.depth);
            style = style.patch(ss);
        } else if is_match {
            let search_face = editor.theme.face(SE::SearchMatch);
            let ss = crate::compose::face_to_ratatui(&search_face, editor.depth);
            style = style.patch(ss);
        }
        if let Some(d) = diag_window.iter().find(|d| overlaps(g_from, g_to, d.range.start, d.range.end)) {
            let diag_face = match d.kind {
                wordcartel_core::diagnostics::DiagnosticKind::Spelling =>
                    compose::compose(&editor.theme, editor.depth, &[SE::DiagSpelling]),
                wordcartel_core::diagnostics::DiagnosticKind::Grammar =>
                    compose::compose(&editor.theme, editor.depth, &[SE::DiagGrammar]),
            };
            style = style.add_modifier(diag_face.add_modifier);
            if let Some(uc) = diag_face.underline_color {
                style = style.underline_color(uc);
            }
        }
        let style = text_fg_or_base(style, &editor.theme, editor.depth);
        if run_style != Some(style) && !run.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut run), run_style.unwrap()));
        }
        run_style = Some(style);
        run.push_str(&p.text);
    }
    if !run.is_empty() {
        spans.push(Span::styled(run, run_style.unwrap()));
    }
    spans
}
```
CRITICAL: `text_fg_or_base` stays BEFORE the run-comparison (moving it changes span boundaries →
cell grids). The MarkedBlock patch stays BELOW Selection/Search/Diag. Order Selection → Search →
Diag preserved.

**4.5 — Add `gather_row_ctx` (phases 6–7 verbatim; `has_block`/`block_hidden`/`diag_active` are
locals).** Byte-identical move; do NOT alter logic. The `focus_region` and `hl_window` bodies below
are the complete current literal (pulled from render.rs :281–299 and :312–327); the assembly is
identical to today's inline phase-6/7 code, just returned in a struct:
```rust
/// Snapshot the row loop's per-frame inputs (render() phases 6–7). has_block/block_hidden/
/// diag_active are gather-time locals feeding use_placed/diag_all; only the 12 fields the paint
/// path reads are kept in RowCtx.
fn gather_row_ctx(editor: &Editor) -> RowCtx<'_> {
    let scroll = editor.active().view.scroll;

    // Compute the active focus region once (before the row loop) when focus is on.
    // For Paragraph: use paragraph_range_at at the caret.
    // For Sentence: scope paragraph_range_at first, then sentence_bounds within that window.
    let focus_region: Option<(usize, usize)> = if editor.view_opts.focus {
        let buf = &editor.active().document.buffer;
        let blocks = editor.active().document.blocks();
        let head = nav::head(editor);
        let region = match editor.view_opts.focus_granularity {
            crate::config::FocusGranularity::Paragraph => {
                nav::paragraph_range_at(blocks, buf, head)
            }
            crate::config::FocusGranularity::Sentence => {
                let (ps, pe) = nav::paragraph_range_at(blocks, buf, head);
                let win = buf.slice(ps..pe);
                let (sf, st) = wordcartel_core::textobj::sentence_bounds(&win, head - ps);
                (ps + sf, ps + st)
            }
        };
        Some(region)
    } else {
        None
    };

    // Collect sorted logical line indices from the layout cache.
    let mut sorted_lines: Vec<usize> = editor.active().view.line_layouts.keys().copied().collect();
    sorted_lines.sort_unstable();

    // Gather search match data once before the row loop (avoids repeated borrow).
    // Clone only the viewport-bounded window (O(visible matches)) rather than the
    // full match list (O(total matches)). The search-bar count/ordinal always reads
    // from SearchState directly, so the truncated window does not affect the N/M display.
    let hl_current: Option<wordcartel_core::search::Match> =
        editor.search.as_ref().and_then(|s| s.current());
    let hl_window: Vec<wordcartel_core::search::Match> = match editor.search.as_ref() {
        None => Vec::new(),
        Some(s) if s.matches().is_empty() => Vec::new(),
        Some(s) => {
            let buf = &editor.active().document.buffer;
            let lo = derive::line_start(buf, scroll);
            // Conservative upper bound: the last logical line in the layout cache.
            let max_visible = sorted_lines.last().copied().unwrap_or(scroll);
            let hi = derive::line_start(buf, max_visible + 1);
            // partition_point keeps the sorted invariant; matches are sorted by start,
            // and non-overlapping so end is also non-decreasing.
            let lo_idx = s.matches().partition_point(|m| m.end <= lo);
            let hi_idx = s.matches().partition_point(|m| m.start < hi);
            s.matches()[lo_idx..hi_idx.max(lo_idx)].to_vec()
        }
    };

    let diag_active = editor.active().diagnostics.valid_for(editor.active().document.version);
    let diag_all: &[wordcartel_core::diagnostics::Diagnostic] =
        if diag_active { &editor.active().diagnostics.diagnostics } else { &[] };

    let sel_range = editor.active().document.selection.primary();
    let (sel_from, sel_to) = (sel_range.from(), sel_range.to());
    let has_sel = !sel_range.is_empty();

    let marked_block = editor.active().marked_block;
    let block_hidden = marked_block.is_some_and(|b| b.hidden);
    let has_block = marked_block.is_some() && !block_hidden;

    let use_placed = !hl_window.is_empty() || diag_active || has_sel || has_block;
    let plain_source = editor.active().view.mode == crate::editor::RenderMode::SourcePlain;

    RowCtx {
        scroll, focus_region, sorted_lines, hl_current, hl_window, diag_all,
        sel_from, sel_to, has_sel, marked_block, use_placed, plain_source,
    }
}
```
The `focus_region` and `hl_window` bodies are moved verbatim (same expressions, same order) from
their current inline positions. `block_hidden`/`has_block`/`diag_active` are locals here and are
NOT stored (Codex finding).

**4.6 — Add `paint_rows` (phase 8: the loop driver).**
```rust
/// Phase 8 — the visible-row paint loop. Owns screen_row, the outer/inner loop,
/// row_dim, fold marker, the segs/placed selector, fold-marker insert, and the
/// per-row render_widget. tg is passed in (single-call invariant) — never recompute.
fn paint_rows(frame: &mut Frame, editor: &Editor, area: Rect,
              edit_top: u16, edit_height: u16, tg: &nav::TextGeometry) {
    let ctx = gather_row_ctx(editor);
    let mut screen_row: u16 = 0;
    'outer: for &l in &ctx.sorted_lines {
        if l < ctx.scroll { continue; }
        let (visual_rows, map) = &editor.active().view.line_layouts[&l];
        let skip_rows = if l == ctx.scroll { editor.active().view.scroll_row } else { 0 };
        for (row_index, vr) in visual_rows.iter().enumerate() {
            if row_index < skip_rows { continue; }
            if screen_row >= edit_height { break 'outer; }

            let row_dim = if let Some((from, to)) = ctx.focus_region {
                let buf = &editor.active().document.buffer;
                let line_off = derive::line_start(buf, l);
                let g_from = line_off + vr.src_span.start;
                let g_to = line_off + vr.src_span.end;
                !row_is_active(g_from, g_to, from, to)
            } else { false };

            let fold_marker_n: Option<usize> = if row_index == skip_rows {
                fold_marker_for(editor, l)
            } else { None };

            let mut spans = if !ctx.use_placed {
                row_spans_segs(editor, &ctx, vr, map, row_dim)
            } else {
                row_spans_placed(editor, &ctx, l, row_index, vr, map, row_dim)
            };

            if let Some(n) = fold_marker_n {
                spans.insert(0, Span::styled("▸ ", compose::compose(&editor.theme, editor.depth, &[SE::FoldMarker])));
                spans.push(Span::styled(
                    format!("  … {n} lines"),
                    compose::compose(&editor.theme, editor.depth, &[SE::FoldMarker]).add_modifier(Modifier::DIM),
                ));
            }

            let line_widget = Line::from(spans);
            let row_area = Rect::new(area.x + tg.text_left, edit_top + screen_row, tg.text_width, 1);
            frame.render_widget(Paragraph::new(line_widget), row_area);
            screen_row += 1;
        }
    }
}
```
Note `map` binds `&ColMap` (`&editor.active().view.line_layouts[&l]` yields `&(Vec<VisualRow>,
ColMap)`); `row_spans_segs`/`row_spans_placed` take `&VisualRow`/`&ColMap` — matches. The
`editor.active()` reborrows inside the loop are identical to today (not a single snapshot).

**4.7 — Reduce `render()` to the skeleton; remove its `#[allow]`.**
Delete phases 6–7 (now in `gather_row_ctx`) and phase 8's inline loop (now `paint_rows`) from
`render()`. The row-region of `render()` becomes the single call
`paint_rows(frame, editor, area, edit_top, edit_height, &tg);` placed exactly where the loop was
(AFTER the wrap-guide, BEFORE `ChromeStyles::build`). Remove `#[allow(clippy::too_many_lines)]`
from `render` (:215). If `render()` still exceeds 100 lines after the split, hoist the wrap-guide
block into `fn paint_wrap_guide(frame, editor, area, edit_top, edit_height, tg)` rather than
re-adding the allow — verify with the clippy run in 4.8.

**4.8 — Verify (expect PASS — golden grids UNCHANGED; the hard gate).**
```
cargo test -p wordcartel --lib render::
cargo test -p wordcartel --lib e2e::
cargo test -p wordcartel --lib
cargo test -p wordcartel --test module_budgets           # render.rs prod ≤ 900
cargo build -p wordcartel 2>&1 | grep -E "warning|error" || echo "warning-free"
cargo clippy --workspace --all-targets 2>&1 | grep -E "warning|error" || echo "clippy clean"
```
Expected: all 83 render tests + 25 e2e journeys byte-identical green — specifically the named
goldens `golden_default_scrollbar_styled`, `golden_list_bullet_darkgray_dim`,
`golden_blockquote_glyph…`, `golden_fold_marker_darkgray`, `golden_wrap_guide_darkgray`, plus the
a11y/canvas/status/dropdown suites and the FocusDim/dim-row cases (guarding the 4-element compose).
`module_budgets` green (render.rs prod well under 900). No `too_many_lines` on `render` (allow
gone), no `too_many_arguments`, warning-free (a retained-but-unread `RowCtx` field would fail here —
proves `has_block` is correctly a local).

**4.9 — Commit (on request):** `refactor(render): H14b — split render() row loop + unify segs/placed`.

---

## Final gates (whole branch, before merge)

Run after all four tasks land:
```
cargo test                                              # every suite, both crates
cargo build 2>&1 | grep -E "warning|error" || echo "warning-free"
cargo test --no-run 2>&1 | grep -E "warning|error" || echo "test build warning-free"
cargo clippy --workspace --all-targets 2>&1 | grep -E "warning|error" || echo "clippy clean"
cargo test -p wordcartel --test module_budgets
bash scripts/smoke/run.sh                               # mandatory-run, advisory-pass — quote the summary
```
Then the two effort gates per CLAUDE.md: a Fable whole-branch review (cross-task invariants,
compiling probes against the real branch) and a Codex pre-merge GO/NO-GO. Merge `--no-ff` to trunk
only when both pass and the human asks; delete the branch; push only when asked.

## History
- 2026-07-10 — drafted (design-author thread) from the Codex-corrected spec + the three map briefs.
