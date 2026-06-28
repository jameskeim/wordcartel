# Wordcartel Effort 9B — WordStar Keymap Fidelity — Design

**Status:** design (brainstormed 2026-06-27)
**Roadmap:** Effort 9B, exec #2, pre-1.0 (`docs/superpowers/plans/2026-06-22-wordcartel-coverage-ledger.md`).
**Goal:** make the opt-in `wordstar` keymap preset a **faithful** WordStar control-diamond
experience — fix the shipped `^A`/`^F` word-motion bug, wire the full `^Q`/`^K` prefix
families, add numbered bookmarks, and build the handful of editor commands faithful
WordStar needs that don't exist yet — while keeping modern arrow/Home/End/Shift-select
keys and never touching the `cua` default.

References: sfwriter.com/wordstar.htm (Sawyer), wordtsar.ca/using-wordtsar.

---

## 1. Scope & philosophy

- **Opt-in only.** All keybinding changes live in the **`wordstar` preset table**
  (`keymap.rs` `WORDSTAR`). The `cua` default is **untouched**. A user gets WordStar
  only via `keymap.preset = "wordstar"`.
- **New commands are first-class registry citizens.** Every new command registers in the
  §10.4 name-keyed registry, so it is **palette-reachable and config-bindable in every
  preset** (`cua`, `wordstar`, or user `[[keymap.patches]]`). The preset table only sets
  the *default* WordStar chords; config can rebind/unbind any of them. (Unknown id in a
  patch → warning + skip; Esc-first chord → rejected, Esc reserved for cancel/dismiss.)
- **Faithful** WordStar diamond + `^Q`/`^K` prefixes, **plus** the modern keys that
  postdate WordStar-era keyboards: **arrows, Home/End, Shift+arrow/Home/End selection**
  (kept from today's `WORDSTAR` table — they extend the diamond, they don't violate it).
- **Decision (locked): build everything (B2).** Faithful coverage includes the commands
  that need building: `delete_line`, `delete_to_line_end`, `save_and_quit`, numbered
  bookmarks, `move_screen_top`/`move_screen_bottom`, `scroll_line_up`/`scroll_line_down`.
- **Excluded by the markdown-source boundary / deferrals:** `^B` paragraph reflow
  (violates source-as-is), `^P` print controls, `^O` on-screen format, dot commands,
  `^J` help (no help system yet); block ops `^KB/^KK/^KC/^KV/^KY/^KW/^KH` → **Effort 9A**;
  file-read/insert `^KR` → **Effort 7**.
- **Terminal-reserved keys not rebound** (their named keys already cover them): `^H`=Backspace,
  `^I`=Tab, `^M`=Enter, `^[`=Esc.

---

## 2. Binding map for the `wordstar` preset

**Prefix convention (locked):** for the `^Q` and `^K` prefixes, the **second key is
accepted with ctrl held *or* plain** — both `^K ^S` and `^K S` resolve to `save`. Each
such command therefore gets **two** preset rows (e.g. `"ctrl-k ctrl-s"` and `"ctrl-k s"`).
Bookmarks are digit-only (`^K 0`), one row each.

**Exception (Codex):** the ctrl-held second-key form is **omitted** when that letter would
form a terminal-reserved control code — `m`→CR (Enter), `j`→LF, `h`→Backspace, `i`→Tab,
`[`→Esc — because many terminals deliver e.g. `Ctrl-M` as `Return`, not `Char('m')+CTRL`.
In practice this affects only **`^KM`** and **`^KJ`** (kept char-mark set/jump): bind
**plain-only** (`ctrl-k m`, `ctrl-k j`), never `ctrl-k ctrl-m`/`ctrl-k ctrl-j`. All `^Q`
sub-letters (S/D/R/C/E/X/F/A/L/P) and the other `^K` letters (S/D/X/Q) are collision-free
and get both forms.

**Preset must never bind `^K` or `^Q` as a one-chord command** — they exist only as
prefixes; a one-chord binding would shadow every two-chord sequence beneath it (exact
match beats prefix in `trie.resolve`). A dedicated test asserts this (§5).

### Cursor diamond (single `^`)
| Key | Action | Target id | Status |
|---|---|---|---|
| `^E` / `^X` | up / down line | `move_up` / `move_down` | exists |
| `^S` / `^D` | left / right char | `move_left` / `move_right` | exists |
| `^A` / `^F` | left / right **word** | `move_word_left` / `move_word_right` | **bug fix** |
| `^R` / `^C` | page up / down | `move_page_up` / `move_page_down` | exists |
| `^W` / `^Z` | scroll line up / down | `scroll_line_up` / `scroll_line_down` | **new** |

### `^Q` "quick" prefix (ctrl-held or plain second key)
| Key | Action | Target id | Status |
|---|---|---|---|
| `^QS` / `^QD` | line start / end | `move_line_start` / `move_line_end` | exists |
| `^QR` / `^QC` | doc start / end | `move_doc_start` / `move_doc_end` | exists |
| `^QE` / `^QX` | top / bottom of screen | `move_screen_top` / `move_screen_bottom` | **new** |
| `^QF` / `^QA` | find / replace | `find` / `replace` | exists |
| `^QL` | repeat find | `find_next` | exists |
| `^QP` | previous position | `jump_back` | exists |
| `^Q0`–`^Q9` | jump bookmark 0–9 | `jump_bookmark_0`..`jump_bookmark_9` | **new** |

### `^K` "block/file" prefix (ctrl-held or plain second key)
| Key | Action | Target id | Status |
|---|---|---|---|
| `^KS` / `^KD` | save / save-done | `save` | exists |
| `^KX` | save **and** exit | `save_and_quit` | **new** |
| `^KQ` | quit (abandon) | `quit` | exists |
| `^K0`–`^K9` | set bookmark 0–9 | `set_bookmark_0`..`set_bookmark_9` | **new** |
| `^KM` / `^KJ` | set / jump char-mark (kept; **plain-only** — see exception) | `set_mark` / `jump_to_mark` | exists |
| `^KC` / `^KV` | copy / paste (**interim**) | `copy` / `paste` | exists (kept) |

### Delete / edit / undo (single `^`)
| Key | Action | Target id | Status |
|---|---|---|---|
| `^G` | delete char forward | `delete_forward` | exists |
| `^T` | delete word right | `delete_word_forward` | exists |
| `^Y` | delete line | `delete_line` | **new** |
| `^QY` | delete to end of line | `delete_to_line_end` | **new** |
| `^U` | undo | `undo` | exists |
| `ctrl-shift-u` | redo (no native WordStar key; locked pairing) | `redo` | exists |

### Kept modern keys (already in the table — retained)
arrows → `move_left/right/up/down`; Home/End → `move_line_start/end`;
Shift+arrows/Home/End → `select_left/right/up/down/line_start/line_end`;
Backspace/Del/Enter → `backspace`/`delete_forward`/`insert_newline`.

**Row-removal note (explicit — Codex):** faithful moves undo to `^U`, freeing `^Z`; `^Z`
(and `^W`) become line-scroll, and `^Y` becomes `delete_line`. The following **current**
`WORDSTAR` rows are therefore **removed/replaced** — each is a behavior change to call out
in the plan:
- `("ctrl-z","undo")` → removed (`^Z` = `scroll_line_down`; undo moves to `^U`).
- `("ctrl-y","redo")` → removed (`^Y` = `delete_line`; redo moves to `ctrl-shift-u`).

**`^KC`/`^KV` interim decision:** WordStar's `^KC`/`^KV` are *block* copy/move, which arrive
in **Effort 9A**. Until then, the existing `("ctrl-k ctrl-c","copy")`/`("ctrl-k ctrl-v","paste")`
rows are **kept** (now also with the plain second-key form) so `wordstar` users aren't left
with no copy/paste — flagged as **interim; 9A reclaims `^KC`/`^KV` for block copy/move**.

---

## 3. New-command semantics

### 3.1 Editing (synchronous, undoable — model on `delete_word_forward`)
- **`delete_line`** (`^Y`): delete the caret's whole **logical** line including its
  trailing newline. Caret lands at the start of the line that slides up (or doc end).
  One undoable changeset; clears the selection and resets `desired_col`. Operates on the
  caret-head's line regardless of any selection.
  **Byte ranges (Codex — derive from raw line starts, NOT `line_text().len()` which strips
  the newline):**
  - normal line `L`: delete `line_start(L)..line_start(L+1)`.
  - *last line with no trailing newline*: delete the **preceding** newline too —
    `line_start(L).saturating_sub(1)..buffer.len()` (so the line vanishes; caret to new end).
  - *single-line doc*: `0..len` → empty buffer, caret at 0.
- **`delete_to_line_end`** (`^QY`): delete from caret to the end of the current logical
  line (the newline is **kept**) — range `caret..line_content_end(L)`. Caret already at
  line end → **true no-op**: return `Handled` **without** applying an empty changeset
  (the plan must confirm the changeset/`apply` path's behavior on an empty range and avoid
  it rather than rely on it).

### 3.2 File (async-aware — **reuse the existing mechanism**)
**Codex correction:** save-and-quit is **already implemented** and tested — do **not** add a
new field. `Editor` already has `quit_after_save: Option<u64>` and `quit_after_save_at:
Option<u64>` (editor.rs:186-187). The quit-confirmation prompt's `PromptAction::SaveAndQuit`
arm (app.rs:284-296) already: captures `version`, calls `dispatch_save`, and arms
`quit_after_save = Some(version)` / `quit_after_save_at = now` **only if** a save job was
actually dispatched (`path.is_some() && prompt.is_none()`). Completion is in the
`Msg::JobDone` → `apply_result` path, which quits when `quit_after_save == Some(version)`
(app.rs:121); a **timeout** (`SAVE_QUIT_TIMEOUT_MS`, app.rs:1566-1576) clears the arm if the
save never completes. Existing tests:
`save_and_quit_sets_quit_after_save_and_exits_on_matching_result`,
`save_and_quit_on_unnamed_buffer_does_not_arm_quit_after_save`.

- **`save_and_quit`** (`^KX`): a **new registry command** that invokes this same logic
  **directly** (no quit-confirm prompt). **Factor the body of the app.rs:284 arm** (minus
  the `editor.prompt = None` modal-dismissal) into a shared
  `pub(crate) fn dispatch_save_and_quit(ctx: &mut Ctx)` (in `save.rs`); the command handler
  calls it, and the `PromptAction::SaveAndQuit` arm is refactored to call it too (DRY). No
  new field, no new completion hook, no change to the timeout / `JobDone` paths.
  No filename → `dispatch_save` shows its status and dispatches nothing → not armed → stays
  open (correct). (`^KQ` = `quit` abandon; `^KS`/`^KD` = `save`.)

### 3.3 Viewport navigation (synchronous, column-preserving via `desired_col`)
- **`move_screen_top`** (`^QE`): move caret to the first visible logical line
  (`view.scroll`), preserving the desired column (clamped to that line).
- **`move_screen_bottom`** (`^QX`): move caret to the last **fully visible** logical line,
  preserving the desired column. **"Fully visible" (Codex — define for soft-wrap):** walk
  visible logical lines from `(view.scroll, view.scroll_row)`, summing each line's visual-row
  count (`line_layouts`/`rows_of_line`); the target is the **last logical line whose final
  visual row still fits within `view.area` height** (a line whose wrapped rows are partially
  clipped at the bottom does **not** count). If only one (partial) line is visible, target =
  `view.scroll`. Reuse the same visible-row walk `ensure_visible` uses (do not re-derive).

### 3.4 Scroll (synchronous, caret-stays-visible — **reuse existing primitives**)
**Codex correction:** `nav.rs` already has `scroll_up_one`/`scroll_down_one` (nav.rs:573)
that scroll by one **visual row** (handling soft-wrap correctly via `scroll`/`scroll_row`).
WordStar `^W`/`^Z` scroll one row; **adopt visual-row scrolling** (it fits the existing model;
logical-line scrolling would diverge under soft-wrap).
- **`scroll_line_up`** (`^W`) / **`scroll_line_down`** (`^Z`): call the existing
  `scroll_up_one`/`scroll_down_one` (clamped to bounds). The **caret does not move** unless
  it would fall outside the viewport, in which case nudge it the **minimum** needed to stay
  visible (WordStar behavior); `desired_col` unchanged when the caret doesn't move. The plan
  confirms whether `scroll_*_one` already keeps the caret visible or whether a post-scroll
  caret-clamp must be added.

### 3.5 Numbered bookmarks (synchronous; share the edit-tracking mark store)
The mark store is `editor.active().marks: BTreeMap<char, usize>` on `Buffer`; `Buffer::apply`
remaps marks through `change::map_pos`. Numbered bookmark `N` **is** char-mark `'N'` (digit
char) — so `^K5` and the interactive `set_mark`→`5` address the **same** slot (intended).
**Edit-tracking guarantee (Codex):** bookmarks survive **normal edits** (which go through
`Buffer::apply` → `map_pos`), exactly like existing point-marks — but **undo/redo do not
remap marks** (only direct `apply` does), so this matches existing mark behavior, no more.
- **`set_bookmark_0`..`set_bookmark_9`** (`^K0`–`^K9`): store caret head under char-mark
  `'0'`–`'9'`; status `"bookmark N set"`. **Non-interactive** (no `pending_mark` step).
- **`jump_bookmark_0`..`jump_bookmark_9`** (`^Q0`–`^Q9`): if the slot is set →
  `marks::record_jump(origin)` then fold-aware jump (`place_caret_visible(UnfoldTo)`) +
  `derive::rebuild` + `nav::ensure_visible`; else status `"no bookmark N"`.
- **Implementation:** factor the non-interactive cores out of `marks::resolve_pending`
  into reusable helpers `set_char_mark(editor, ch)` / `jump_char_mark(editor, ch)`;
  `resolve_pending` then calls them (DRY). Register the 20 commands via two loops over
  `'0'..='9'`, each closure capturing its digit.

---

## 4. Architecture / files touched

| File | Change |
|---|---|
| `wordcartel/src/keymap.rs` | Extend the `WORDSTAR` table: the `^A`/`^F` fix; the full diamond/`^Q`/`^K` rows (both ctrl-held and plain prefix forms, **except** `^KM`/`^KJ` plain-only); `^U`/`ctrl-shift-u` undo/redo; **remove** the stale `("ctrl-z","undo")` and `("ctrl-y","redo")` rows; keep `^KC`/`^KV`→copy/paste (interim). CUA untouched. |
| `wordcartel/src/commands.rs` | New `Command` variants + `run()` arms: `DeleteLine`, `DeleteToLineEnd`, `MoveScreenTop`, `MoveScreenBottom`, `ScrollLineUp`, `ScrollLineDown`. (`save_and_quit` is a registry closure calling `save::dispatch_save_and_quit`, not a `Command`.) |
| `wordcartel/src/registry.rs` | Register the new editing ids (Edit), viewport/scroll ids (View), `save_and_quit` (File), and the 20 bookmark ids via two `'0'..='9'` loops (View). Set appropriate `MenuCategory`. |
| `wordcartel/src/marks.rs` | Factor `set_char_mark(editor, ch)` / `jump_char_mark(editor, ch)`; `resolve_pending` reuses them. |
| `wordcartel/src/nav.rs` | `move_screen_top/bottom` (visible-rows walk from `view.scroll`/`scroll_row`); `scroll_line_up/down` wrap the **existing** `scroll_up_one`/`scroll_down_one`, keeping the caret visible. |
| `wordcartel/src/save.rs` | **Factor** `pub(crate) fn dispatch_save_and_quit(ctx: &mut Ctx)` from the existing `PromptAction::SaveAndQuit` arm body (capture version → `dispatch_save` → arm `quit_after_save`/`_at` iff a job was dispatched). The `save_and_quit` command calls it. |
| `wordcartel/src/app.rs` | Refactor the `PromptAction::SaveAndQuit` arm (app.rs:284) to call the factored `dispatch_save_and_quit` (DRY). **No change** to `apply_result` quit-on-version (app.rs:121) or the `SAVE_QUIT_TIMEOUT_MS` timeout (app.rs:1566) — completion/failure already handled. **No new field** (`quit_after_save: Option<u64>` already exists). |

**Isolation:** keybindings (`keymap.rs`) are pure data; each new command is a small,
independently-testable unit; `save_and_quit` reuses the already-built, already-tested
version-armed quit-after-save mechanism (a one-function factor + DRY refactor); bookmarks
reuse the existing edit-tracking mark store. No `wordcartel-core` change (all edits go
through the shell buffer/changeset path).

---

## 5. Testing

- **keymap (`keymap.rs` tests):**
  - `both_presets_resolve_against_builtins` already iterates `wordstar` — it now validates
    **every new id resolves** in the registry (guards typos/missing registrations).
  - `^A`/`^F` resolve to `move_word_left`/`move_word_right` (regression test for the fix).
  - `^Q`/`^K` prefixes resolve with **both** the ctrl-held and plain second key
    (`^K ^S` and `^K S` → `save`; `^Q F` / `^Q ^F` → `find`).
  - `^KM`/`^KJ` resolve **only** via the plain form (`ctrl-k m`/`ctrl-k j`); assert
    `ctrl-k ctrl-m` is **not** a wordstar binding (reserved-key exception).
  - bookmark chords resolve (`^K 0` → `set_bookmark_0`, `^Q 9` → `jump_bookmark_9`).
  - `^U` → `undo`, `ctrl-shift-u` → `redo`; assert the **removed** rows are gone
    (`ctrl-z` no longer `undo`; `ctrl-y` no longer `redo` — `ctrl-z`→`scroll_line_down`,
    `ctrl-y` unbound or per table).
  - **NEW dedicated `wordstar` chord-set test (Codex):** enumerate every `WORDSTAR` chord
    sequence and assert (a) **no duplicate chord** maps to two ids, and (b) **no bound
    sequence is a strict prefix of another** — i.e. `^K`/`^Q` are never bound as one-chord
    commands that would shadow their sub-sequences. (`both_presets_resolve_against_builtins`
    covers id-resolution but not collisions/shadowing.)
- **editing (`commands.rs` tests):** `delete_line` mid-doc, last line w/o trailing newline,
  single-line doc → empty (assert exact resulting buffer + caret); `delete_to_line_end`
  mid-line, and at-line-end → no-op (buffer **byte-identical**, version unchanged → confirms
  no empty changeset was applied).
- **viewport nav (`nav.rs` tests):** with a known `view.scroll`/area, `move_screen_top`
  lands on the first visible line and `move_screen_bottom` on the last **fully** visible line
  (incl. a soft-wrapped case where the bottom line is partially clipped → excluded), column
  preserved; `scroll_line_up`/`down` move the viewport by one visual row and keep the caret
  within the viewport (caret nudged only when it would leave; otherwise unmoved, `desired_col`
  unchanged).
- **save_and_quit (reuse — `app.rs`/`save.rs` tests):** the factored `dispatch_save_and_quit`
  arms `quit_after_save = Some(version)` when a job is dispatched, and does **not** arm on a
  no-filename buffer (mirror the existing `save_and_quit_sets_quit_after_save_and_exits_on_matching_result`
  and `save_and_quit_on_unnamed_buffer_does_not_arm_quit_after_save` tests); add a test that
  the `save_and_quit` **command** path (via registry) reaches the same armed state as the
  prompt path (proves the DRY factor). Completion/timeout already covered by existing tests.
- **bookmarks (`marks.rs` tests):** `set_bookmark_N`/`jump_bookmark_N` round-trip; jump to an
  unset slot → `"no bookmark N"` status, no move; bookmark slot shared with interactive
  `set_mark`→`'N'`; `jump_bookmark_N` records jump-back and unfolds a folded target
  (fold-aware), mirroring the existing `jump_to_mark_into_fold_reveals_target` test.

---

## 6. Out of scope (explicitly deferred)

- Block operations `^KB/^KK/^KC/^KV/^KY/^KW/^KH` (persistent marked blocks) → **Effort 9A**.
- File-read/insert `^KR`, save-as for `^KX` on an untitled doc → **Effort 7**.
- `^B` paragraph reflow, `^P` print controls, `^O` on-screen format, dot commands,
  `^J` help — out by the markdown-source-as-is design boundary / no subsystem yet.
- Overtype/insert toggle (`^V`) — wordcartel is always-insert; no overtype mode.
- WordStar-style on-screen menus / the classic help-level system.
