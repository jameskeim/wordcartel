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
| `^KM` / `^KJ` | set / jump char-mark (kept) | `set_mark` / `jump_to_mark` | exists |

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

**Conflict note:** faithful moves undo to `^U`, which frees `^Z`; `^Z` (and `^W`) become
line-scroll, per the diamond. The existing `WORDSTAR` rows `("ctrl-z","undo")` and
`("ctrl-y","redo")` are **removed/replaced** accordingly (`^Y` becomes `delete_line`).

---

## 3. New-command semantics

### 3.1 Editing (synchronous, undoable — model on `delete_word_forward`)
- **`delete_line`** (`^Y`): delete the caret's whole **logical** line including its
  trailing newline. Caret lands at the start of the line that slides up (or doc end).
  One undoable changeset; clears the selection and resets `desired_col`. Edge cases:
  *last line with no trailing newline* → remove the line **and** the preceding newline
  so the line vanishes; *single-line doc* → empties the buffer (caret at 0). Operates on
  the caret-head's line regardless of any selection.
- **`delete_to_line_end`** (`^QY`): delete from caret to the end of the current logical
  line (the newline is **kept**). Caret already at line end → no-op (no empty changeset).

### 3.2 File (async-aware)
- **`save_and_quit`** (`^KX`): `save` is **asynchronous** (`save.rs::dispatch_save` starts
  a background save; success/failure arrives later as a message). So this command:
  1. If save **cannot start** (no file name — current `dispatch_save` shows a
     "No file name…" status; note the literal still references the old effort number
     for save-as) → do **not** arm; leave that status; stay open.
  2. Otherwise **arm** `editor.quit_after_save = true` and dispatch `save`.
  3. The **save-completion message handler** (where `saved_version`/status are set today):
     on **success** → perform the quit; on **failure** → clear `quit_after_save` and leave
     the error status (no data loss, no surprise exit).
  This mirrors the existing `pending_export` arm-then-complete pattern (Effort 4c).
  (`^KQ` = `quit` abandon; `^KS`/`^KD` = `save`.)

### 3.3 Viewport navigation (synchronous, column-preserving via `desired_col`)
- **`move_screen_top`** (`^QE`): move caret to the first visible logical line
  (`view.scroll`), preserving the desired column (clamped to that line).
- **`move_screen_bottom`** (`^QX`): move caret to the last **fully** visible logical line,
  preserving the desired column. Computed from `view.scroll`/`view.scroll_row` + the
  visible-rows walk the renderer/`ensure_visible` already use (`line_layouts` rows per
  logical line within `view.area` height).

### 3.4 Scroll (synchronous, caret-stays-visible)
- **`scroll_line_up`** (`^W`) / **`scroll_line_down`** (`^Z`): adjust `view.scroll` by one
  logical line (clamped to `[0, total_logical_lines-1]`; respects folds via the same
  `normalize_line` `ensure_visible` uses). The **caret does not move** unless it would fall
  outside the viewport, in which case nudge the caret the **minimum** needed to stay
  visible (WordStar behavior). Does not change `desired_col` when the caret doesn't move.

### 3.5 Numbered bookmarks (synchronous; share the edit-tracking mark store)
The mark store is `editor.active().marks: BTreeMap<char, usize>` (edit-tracking via
`map_pos`). Numbered bookmark `N` **is** char-mark `'N'` (digit char) — so `^K5` and the
interactive `set_mark`→`5` address the **same** slot (intended).
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
| `wordcartel/src/keymap.rs` | Extend the `WORDSTAR` table: the `^A`/`^F` fix; the full diamond/`^Q`/`^K` rows (both ctrl-held and plain prefix forms); `^U`/`ctrl-shift-u` undo/redo; remove the stale `("ctrl-z","undo")`/`("ctrl-y","redo")` rows. CUA untouched. |
| `wordcartel/src/commands.rs` | New `Command` variants + `run()` arms: `DeleteLine`, `DeleteToLineEnd`, `MoveScreenTop`, `MoveScreenBottom`, `ScrollLineUp`, `ScrollLineDown`. (`save_and_quit` is a registry closure, not a `Command` — it touches the editor + save dispatch directly.) |
| `wordcartel/src/registry.rs` | Register the new editing ids (Edit), viewport/scroll ids (View), `save_and_quit` (File), and the 20 bookmark ids via two `'0'..='9'` loops (View). Set appropriate `MenuCategory`. |
| `wordcartel/src/marks.rs` | Factor `set_char_mark(editor, ch)` / `jump_char_mark(editor, ch)`; `resolve_pending` reuses them. |
| `wordcartel/src/nav.rs` | `move_screen_top/bottom` (read `view.scroll`/`scroll_row` + visible-rows walk) and `scroll_line_up/down` (adjust `view.scroll`, keep caret visible). |
| `wordcartel/src/editor.rs` | Add `quit_after_save: bool` field (init `false` in `new_from_text`). |
| `wordcartel/src/save.rs` (and/or the save-completion handler in `app.rs`) | On save success, if `quit_after_save` → quit; on failure → clear `quit_after_save`. |

**Isolation:** keybindings (`keymap.rs`) are pure data; each new command is a small,
independently-testable unit; the async `save_and_quit` reuses the established
arm-then-complete pattern; bookmarks reuse the existing edit-tracking mark store. No
`wordcartel-core` change (all edits go through the shell buffer/changeset path).

---

## 5. Testing

- **keymap (`keymap.rs` tests):**
  - `both_presets_resolve_against_builtins` already iterates `wordstar` — it now validates
    **every new id resolves** in the registry (guards typos/missing registrations).
  - `^A`/`^F` resolve to `move_word_left`/`move_word_right` (regression test for the fix).
  - `^Q`/`^K` prefixes resolve with **both** the ctrl-held and plain second key
    (`^K ^S` and `^K S` → `save`; `^Q F` / `^Q ^F` → `find`).
  - bookmark chords resolve (`^K 0` → `set_bookmark_0`, `^Q 9` → `jump_bookmark_9`).
  - `^U` → `undo`, `ctrl-shift-u` → `redo`; no `wordstar` chord collisions (each chord/prefix
    sequence resolves to exactly one command or is a clean prefix).
- **editing (`commands.rs` tests):** `delete_line` mid-doc, last line w/o trailing newline,
  single-line doc → empty; `delete_to_line_end` mid-line, and at-line-end → no-op (buffer
  unchanged, no changeset).
- **viewport nav (`nav.rs` tests):** with a known `view.scroll`/area, `move_screen_top`
  lands on the first visible line and `move_screen_bottom` on the last fully-visible line,
  column preserved; `scroll_line_up`/`down` shift `view.scroll` by one and keep the caret
  within the viewport (caret nudged only when it would leave; otherwise unmoved).
- **save_and_quit:** arming requires a file name (no-filename path does **not** arm and
  shows save's status); on a simulated save-**success** message with `quit_after_save` set →
  quit performed; on save-**failure** → `quit_after_save` cleared and editor stays open with
  the error status. (Mirror how existing save-message tests drive the completion handler.)
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
