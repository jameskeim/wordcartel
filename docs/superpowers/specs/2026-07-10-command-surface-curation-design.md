# Command-Surface Curation — Design Spec

**Date:** 2026-07-10
**Status:** spec for review (Codex gate). ONE effort, internally phased.
**Sources of truth:** the approved brainstorm decisions ledger (`curation-decisions.md`) and the
Phase-0 surface map (`command-surface-map.md`), both grounded against the real source in this
document. Backlog items covered: A11, A10, A12, A3b, A9, A14, A8, A13.
**Contract:** this effort IS a command-surface effort — per `docs/design/command-surface-contract.md`
this spec states conformance item by item (see the dedicated section) and Phase 4 WILL AMEND the
contract (dynamic menu sections, with a new History entry) as a scheduled implementation task —
the contract file is unamended at spec stage and is edited only when Phase 4 lands.

Phase order (dependency order, also the plan's execution order):
Phase 1 A11 scope convention → Phase 2 menu organization (A10, A12) → Phase 3 mechanical
(A3b, A9, A14) → Phase 4 A8 contract amendment + Documents menu → Phase 5 A13 mouse parity.

Grounding corrections already folded into the approved design (2026-07-10): A13's real gap is the
minibuffer + search overlays (theme picker / file browser / outline already have click support);
A14 routes through `editor.apply` + `settle_after_edit`, NOT `submit_transaction` (that is the M2
untrusted/plugin boundary); the registry holds ~156 commands / ~72 menu-listed (not ~126/~58).

---

## Phase 1 — A11: the scope convention, `select_marked_block`, filter → shell

### 1.1 The scope convention (A11.1, A11.2 — design law, documented not re-implemented)

**Convention:** every scope-taking command is **selection-primary with a per-family
empty-selection fallback**; the persistent marked block (`MarkedBlock { start, end, hidden }`,
stored as `Buffer::marked_block: Option<MarkedBlock>` in `wordcartel/src/editor.rs`) stays
**orthogonal — an explicit target only, never implicit scope**. Non-empty-selection and
explicit-target behavior is uniform across all scope-taking commands; only the empty-selection
fallback matches each family's natural unit:

- **Transforms** → structural block at caret. Already shipped (C2):
  `transform::region_for_transform(doc)` in `wordcartel/src/transform.rs` returns
  `transform_unit_at(...)` for an empty primary selection, else
  `snap_to_blocks(...)` over the selection. `dispatch_transform(editor, kind, region, clock,
  msg_tx)` calls it when `region` is `None`; the `_buffer` variants pass `Some(0..len)`.
  **No code change** — verified conformant.
- **Filter** → whole buffer. Already shipped: `prompts::submit_filter_line` builds
  `FilterSpec { input: Input::SelectionElseBuffer, .. }` (`wordcartel/src/filter.rs` —
  selection if non-empty, else the whole buffer). **No input-scoping change** — verified
  conformant.
- **A14 case ops** (Phase 3) → word at caret. New instance of the same convention.

The convention is recorded in this spec and carried into module docs (`transform.rs`,
`filter.rs`, the new `select_marked_block` handler) — it is a design convention, not a new
contract law (the contract amendment in Phase 4 is A8's only).

### 1.2 New command: `select_marked_block` (A11.3)

The one block → selection bridge, replacing any per-command `_block` variants (none are added).

- **Behavior:** if `editor.active().marked_block` is `Some(MarkedBlock { start, end, .. })`,
  set the active document's selection to `Selection::range(start, end)` (anchor at `start`,
  head at `end`), then `derive::rebuild(editor)` + `nav::ensure_visible(editor)`. If no block
  is marked, set a status message (family precedent in `blocks_marked.rs`) and do nothing.
  The `hidden` flag is not consulted or changed — the range is real text either way, matching
  the other `blocks_marked` range ops. The marked block itself is NOT cleared or moved.
- **Seams:** new `pub fn select_marked_block(editor: &mut Editor)` in
  `wordcartel/src/blocks_marked.rs`; one `r.register(...)` row in
  `registry.rs::Registry::builtins`.
- **Registration:** id `select_marked_block`, label `"Select Block"`, stateless,
  `menu: Some(MenuCategory::Block)` (the Block category lands in Phase 2 — end state given
  here; the plan sequences the intermediate).
- **Binding:** palette-first — no default chord in either preset (user-bindable; the WordStar
  preset MAY chord the block family — the plan decides).

### 1.3 Filter runs through a shell (A11.4 — SIGNED-OFF behavior change)

Today `submit_filter_line` (`wordcartel/src/prompts.rs`) splits the minibuffer line on
whitespace into argv and passes `shell: false`. The shell mechanism already exists and is
tested at the `filter.rs` layer: `run_subprocess(argv, shell, ...)` builds
`vec!["sh", "-c", argv.join(" ")]` when `shell` is true. This phase flips the interactive
prompt onto it:

- **Behavior:** the submitted line executes via POSIX `sh -c <line>` — pipes, quoting, and
  redirects work (vi `!` / Emacs `shell-command-on-region` model). Input/disposition are
  unchanged: `Input::SelectionElseBuffer`, `Disposition::Filter`.
- **Exact argv rule (correctness-critical):** the raw line is passed as a **single-element
  argv** — `argv: vec![line.to_string()], shell: true` — so `run_subprocess`'s
  `argv.join(" ")` reproduces the line verbatim. Splitting on whitespace and re-joining is
  FORBIDDEN here: it would collapse runs of spaces inside quoted arguments (e.g.
  `sed 's/a  b/c/'`). The empty guard becomes `line.trim().is_empty()` → status message, no
  dispatch (replacing the argv-empty check).
- **Caps and isolation KEPT, unchanged:** `timeout: Duration::from_secs(10)`,
  `max_output: crate::limits::MAX_FILTER_OUTPUT`, and the existing worker panic isolation in
  `filter::dispatch_filter` all stay exactly as they are. The cancel path (Esc → `CancelFlag`)
  is untouched.
- **Trust boundary (documented):** the line is user-typed at an interactive prompt — the same
  trust class as `:!` in vi. This is distinct from the plugin/untrusted edit boundary
  (`transact.rs::submit_transaction`), which is not involved. `submit_filter_line`'s doc
  comment (which currently states `shell: false` is the security default) is rewritten to
  state this boundary.
- **Shell choice:** POSIX `sh` — the mechanism `run_subprocess` already implements. `$SHELL`
  is deliberately not consulted (deterministic across user shells; fish/nushell quoting would
  diverge). The ledger listed both forms; the code's existing mechanism decides.

**Example-filter docs, surfaced from the Filter prompt:** the Filter prompt string changes from
`"> "` to `"sh> "` (signals shell semantics), and while the Filter minibuffer is open with
empty text, the status row renders a ghost example hint after the prompt — a single static
line covering the ledger's example families, e.g.
`e.g. sort | uniq · fmt -w 72 · sed s/a/b/g · tr a-z A-Z · column -t`.
Seams: the hint constant lives in `wordcartel/src/minibuffer.rs`; the render change is one arm
in `render.rs::paint_status` (the minibuffer branch appends the hint when
`mb.kind == MinibufferKind::Filter && mb.text.is_empty()`), styled receded/dim per the chrome
DIM conventions (exact style token is the plan's). The hardware caret (`render.rs::place_cursor`)
already sits at `prompt + text` — the ghost renders after the caret and disappears on the first
typed char.

### 1.4 Test intent (Phase 1)

- `select_marked_block`: unit tests in `blocks_marked.rs` — selects exactly `start..end`;
  marked block survives; no-block case sets status and leaves selection unchanged.
- Shell filter: unit test on `submit_filter_line` — single-element argv, `shell: true`, caps
  fields unchanged, trimmed-empty guard. Integration test through `dispatch_filter` running a
  real quoted pipeline (the existing filter test suite already drives real subprocesses) —
  e.g. a quoted `sed` program containing a double space survives verbatim; a pipe (`tr … | …`)
  produces piped output. Existing timeout/max_output/panic-isolation tests stay green.
- Ghost hint: render test (TestBackend, `renders_active_minibuffer_on_status_row` precedent) —
  hint visible when Filter minibuffer empty, gone once text is non-empty, absent for other
  minibuffer kinds.

---

## Phase 2 — menu organization: A10 (Block menu) and A12 (scratch round-trip)

### 2.1 A10 — new `MenuCategory::Block`, the whole block family moves

- `registry.rs`: `MenuCategory` gains a `Block` variant; `MENU_ORDER` (a literal const,
  currently `[MenuCategory; 6]`) grows to include it **edit-adjacent**. End state after
  Phase 4: `[File, Edit, Block, Format, View, Documents, Settings, Export]` (`[MenuCategory; 8]`).
  `menu.rs::category_label`'s exhaustive match gains `Block => "Block"` (the compiler forces
  every `MenuCategory` match site).
- **Moves — `meta.menu` edits only** on existing `builtins()` rows, all to
  `Some(MenuCategory::Block)`: `block_begin`, `block_end`, `mark_block_from_selection`,
  `block_copy`, `block_move`, `block_delete`, `block_jump_begin`, `block_jump_end`,
  `block_toggle_hidden`, `block_clear` (all currently Edit); `block_write` (currently
  **File** → Block); `copy_block_to_scratch`, `move_block_to_scratch` (currently Edit);
  plus the new `select_marked_block` (Phase 1). Fourteen Block rows total.
- Constraints honored: one `Option<MenuCategory>` per command (no dual listing); flat
  dropdowns (no submenus). `menu::grouped_commands` needs no structural change for this phase —
  it already groups by `meta.menu` over `MENU_ORDER`.

### 2.2 A12 — scratch round-trip + exclusion from rotation

Scratch today is an ordinary `editor.buffers` entry flagged by `editor.scratch_id` /
`Editor::is_scratch`, reachable via `next_buffer`/`prev_buffer` (`workspace::cycle`,
`rem_euclid` over ALL buffers) and the `switch_buffer` MRU switcher
(`workspace::buffer_switch_rows`, which lists scratch as `*scratch*`). Decision: keep
`goto_scratch` (always-go) AND add `toggle_scratch` (round-trip); exclude scratch from
rotation surfaces.

- **New command `toggle_scratch`:**
  - Not on scratch → record the active (ordinary) buffer id, then jump to scratch.
  - On scratch → return to the recorded prior buffer if it still resolves
    (`Editor::by_id`); else fall back to the most-recently-active ordinary buffer (first
    non-scratch id in `editor.mru` that resolves, then buffer-vec order); if no ordinary
    buffer target exists, stay on scratch and set a status hint. (An ordinary buffer always
    exists by the `close_buffer_now` last-ordinary-replacement invariant, so the hint arm is
    a defensive tail, kept for robustness.)
  - **Coherence rule:** entering scratch via EITHER `toggle_scratch` or `goto_scratch`
    records the departing ordinary buffer (shared helper), so a `goto_scratch` →
    `toggle_scratch` sequence round-trips as a user expects. `goto_scratch`'s own behavior is
    otherwise unchanged.
  - Seams: new field `pub scratch_return: Option<BufferId>` on `Editor`
    (`wordcartel/src/editor.rs`, beside `scratch_id`/`mru`); new
    `pub fn toggle_scratch(editor: &mut Editor)` in `wordcartel/src/workspace.rs` built on
    `workspace::switch_to`; registration row in `builtins()` immediately after
    `goto_scratch` (menu order within a category is registration order, so the two sit
    adjacent in View).
  - Registration: id `toggle_scratch`, label `"Toggle Scratch Buffer"`, stateless,
    `menu: Some(MenuCategory::View)` (default home confirmed at spec review per the ledger).
  - Binding: **default-bound in both presets** (exact chords are the plan's, conflict-checked
    per preset trie).
- **Exclusions (scratch reachable only via toggle/goto; it is already un-closable):**
  - `workspace::cycle` skips the scratch buffer: the rotation ring is the ordinary buffers in
    buffer-vec order; cycling from scratch enters the ring at the slot following (next) /
    preceding (prev) the scratch's position. Single-ordinary-buffer cycling stays a no-op.
  - `workspace::buffer_switch_rows` omits the scratch row (the `switch_buffer` palette
    switcher no longer lists `*scratch*`).
  - The A8 Documents menu excludes scratch (Phase 4).
- Single per-editor scratch — model unchanged.

### 2.3 Test intent (Phase 2)

- Menu grouping: `menu.rs` tests — Block group present, edit-adjacent in `MENU_ORDER`, carries
  exactly the fourteen rows; `block_write` no longer in File. Existing registration-order and
  meta tests in `registry.rs` updated for the category moves.
- A12: `workspace.rs` tests — cycle skips scratch (including from-scratch entry point);
  `buffer_switch_rows` has no scratch row; `toggle_scratch` round-trips; closed-prior → MRU
  fallback; recorded-prior via `goto_scratch` entry; defensive hint arm. Existing tests that
  assert scratch IN rotation/switcher (`cycle_wraps_in_stable_order_including_scratch`,
  `switcher_rows_mru_order_with_display_names`) are updated to the new law.

---

## Phase 3 — mechanical: A9, A3b, A14

### 3.1 A9 — `set_wrap_column` becomes a stateful menu row

Today: plain `register`, label `"Set Wrap Column…"`, `Some(MenuCategory::Settings)`; handler
opens `MinibufferKind::WrapColumn`; submit path `prompts::wrap_column_submit` parses, clamps
to 20..=9999, writes `editor.view_opts.wrap_column` (a `u16`), rebuilds. All of that flow is
UNCHANGED — this item only makes the menu row show live state.

- Convert to `register_stateful` templated on `cycle_scrollbar`; the state fn reads
  `e.view_opts.wrap_column`.
- **Menu row (the approved design):** base `Wrap Column`, right-aligned value `80…` — the
  value shows state; the trailing `…` signals it opens the minibuffer prompt (vs cycles,
  which have no `…`).
- **Code-forced adaptation (FLAGGED):** the ledger says "`MenuMark::Value`", but
  `MenuMark::Value` carries `&'static str` (`registry.rs`:
  `pub enum MenuMark { OnOff(bool), Value(&'static str) }`, derives include `Copy`) and the
  wrap column is a runtime `u16` — a formatted `"80…"` cannot be `'static`. **Adaptation:**
  `MenuMark` gains an owned variant, `Text(String)`, and drops `Copy` (keeps `Clone, Debug,
  PartialEq, Eq`). Consumers construct and consume by value (`menu_leaf_parts` matches the
  state fn's return), so `Copy` is load-bearing nowhere; the enum stays exhaustive and the
  new arm in `menu_leaf_parts` is `MenuMark::Text(s) => s`. The design intent (live value in
  the row) is preserved exactly; only the carrier type is extended. Surfaced here per process
  — this is the spec's proposed resolution, subject to the spec review.
- **Label:** the registry label becomes `"Wrap Column: Set…"` — following the existing
  stateful ": variants" label convention (`"Canvas: Opaque/Transparent"`), so
  `menu_leaf_parts`'s split-at-`:` derivation yields base `Wrap Column`, and the palette
  (which shows `meta.label` verbatim) reads "Wrap Column: Set…" with its prompt-signaling
  ellipsis. The state fn returns `MenuMark::Text(format!("{}\u{2026}", e.view_opts.wrap_column))`.
- Id stays `set_wrap_column` (keymap bindings and the law-2 settings test key off the id).
  Binding: unchanged (per the binding policy).

### 3.2 A3b — the item-by-item placement sweep (run during this spec, per the ledger)

The curation principle is already adopted; the sweep below applied the contract's one
judgment ("browse-for-by-category vs palette-only") to all ~156 registrations, on top of the
structural moves owned by A10 (block family) and A8 (buffer-nav rows). Results:

- **Normative (pre-resolved in the ledger):** `filter` moves **Edit → Format** — post-A11 it
  is a text-shaping op, sibling of `reflow`/`unwrap`/`ventilate`; not a block command.
  A `meta.menu` edit on its `builtins()` row.
- **Resolved at spec review (2026-07-10 — move):** `transform` — label `"Transform…"`,
  currently `Some(MenuCategory::View)` — opens the `Prompt::transform_chooser()` over
  reflow/unwrap/ventilate, whose discrete commands are ALL Format. The filter rationale
  applies verbatim; View is a historical accident. **`transform`: View → Format.**
- **Resolved at spec review (2026-07-10 — move):** `delete_word_back`,
  `delete_word_forward`, `delete_line`, `delete_to_line_end` — currently
  `Some(MenuCategory::Edit)`. These are keystroke-native ops, exactly the class the contract
  routes palette-only and exactly the class A14 places palette-only (`join_line`,
  `just_one_space`, …). Keeping four keystroke-native deletes in the Edit menu while the ten
  A14 siblings are palette-only is incoherent. **Recommendation: all four → `menu: None`**,
  leaving Edit as the classic word-processor set (Undo/Redo/Cut/Copy/Paste/Select
  All/Find/Replace). Their default chords are untouched — this is menu placement only.
- **Everything else: confirmed in place.** Motions/selection/bookmarks/diagnostics/scroll/
  heading commands are already `menu: None`; File/Export/Settings rows and the
  View toggles + set-per-state primitive pattern (`scrollbar_*`, `status_line_*`,
  `splash_*`, `menu_bar_*`, `clipboard_provider_*`, `keymap_*` primitives palette-only with
  stateful menu representatives) all conform; no other row changes category.

Both groups were the sweep's "genuine ambiguities surfaced during spec" per the ledger; both
were **confirmed at spec review (2026-07-10)** and are now normative end state.

### 3.3 A14 — ten atomic-edit commands

Ten new commands, all templated on the `commands/edit.rs` shape: compute a contiguous
`(from, to)` region + replacement text; early `CommandResult::Noop` guards; a single
`ChangeSet` (via the shared `replace_changeset(from, to, &text, doc_len)` /
`ChangeSet::delete`) + `Edit { range, new_len }`;
`Transaction::new(cs).with_selection(...)`; **`editor.apply(txn, edit, EditKind::Other,
clock)`** — direct apply, NOT `transact.rs::submit_transaction` (the M2 untrusted boundary,
which validates externally-constructed transactions; internal commands build known-good
transactions); then the shared `settle_after_edit(editor)` epilogue. One transaction each =
one undo step (precedent test: `delete_word_back_is_one_undo_step`). Every op is a single
contiguous replacement, so the one-`Edit` incremental-parse contract holds.

**Placement of code + exact visibility (verified against source).** Today the atomic-edit
handlers in `commands/edit.rs` are reached ONLY through the `commands::run` dispatcher (e.g.
`registry.rs` registers `|c| run(c, Command::DeleteWord { back })`), because `commands.rs`
declares `mod edit;` **privately** and its handlers are `pub(super) fn`. The A14 design
requires direct registration (no `Command`-enum growth — module-structure GATE), so `textops`
must be reachable from `registry.rs`, which lives OUTSIDE the `commands` module. Concrete,
compilable shape:

- New module `wordcartel/src/commands/textops.rs`, declared in `commands.rs` as
  `pub(crate) mod textops;` (NOT private like `mod edit;` — a private module would make
  `crate::commands::textops::…` unreachable from `registry.rs`).
- Each handler is `pub(crate) fn <id>(editor: &mut Editor, clock: &dyn Clock) ->
  CommandResult` (case ops take no extra args; all follow the `commands/edit.rs` return
  shape).
- `registry.rs::builtins` registers each as a direct row, `blocks_marked`-style:
  `r.register("transpose_chars", "…", None, |c| crate::commands::textops::transpose_chars(c.editor, c.clock))`
  (case ops → `Some(MenuCategory::Format)`). No `Command` enum variant, no new
  `commands::run` arm — the dispatcher and hub budgets are untouched.
- **Shared-epilogue visibility.** `textops` reuses two `commands`-module helpers:
  `replace_changeset` (a private `fn` in `commands.rs`) is already reachable — `textops` is a
  child module of `commands`, and a child may reference an ancestor's private items via
  `super::replace_changeset`, so NO change is needed there. `settle_after_edit` is a private
  `fn` in the SIBLING `edit` module, so it is NOT visible to `textops` as-is; widen it to
  `pub(super) fn settle_after_edit` in `commands/edit.rs` (that lands it at `commands` scope,
  reachable from the descendant `textops` as `super::edit::settle_after_edit`). `mod edit;`
  itself stays private (descendants of `commands` still see it). Alternatively `textops` may
  inline the two-line epilogue (`derive::rebuild` + `nav::ensure_visible` + `desired_col =
  None`); sharing `settle_after_edit` is preferred to avoid drift. The plan picks one.

The commands (ids per the ledger). Scope: the three case ops are scope-taking under the A11
convention — non-empty primary selection, else the **word at the caret** (resolved by the
same `nav` word-boundary rules the `delete_word` commands use). The other seven are
caret-anchored point edits; a non-empty selection is not consulted.

| id | menu | behavior |
|---|---|---|
| `transpose_chars` | None | Swap the characters (chars, multibyte-safe) immediately before and after the caret; caret ends after the pair. Noop when either side is missing. |
| `transpose_words` | None | Swap the word before with the word after the caret, preserving the text between them; caret ends after the second word. Noop when two words are not available. |
| `transpose_lines` | None | Swap the caret line with the line above; caret lands at the start of the line below the swapped pair (Emacs `C-x C-t` — repeat-invocation drags a line down). Noop on the first line. |
| `upcase` | Format | Selection-else-word-at-caret → Unicode uppercase (`char::to_uppercase`; length may change, e.g. ß→SS). Label `"Uppercase"`. |
| `downcase` | Format | Same scope → Unicode lowercase. Label `"Lowercase"`. |
| `capitalize` | Format | Same scope → each word: first char uppercased, rest lowercased. Label `"Capitalize"`. |
| `join_line` | None | Join the caret line with the next: the newline plus the next line's leading whitespace becomes a single space (vi `J`); caret at the join. Noop on the last line. |
| `just_one_space` | None | Replace the run of spaces/tabs around the caret with exactly one space (inserting one if none) — Emacs `M-SPC`. |
| `delete_blank_lines` | None | Emacs `C-x C-o`: on a blank line, collapse the surrounding blank run to one blank line; on an isolated blank line, delete it; on a non-blank line, delete all blank lines immediately following. Noop when there is nothing to do. |
| `delete_horizontal_space` | None | Delete all spaces/tabs around the caret — Emacs `M-\`. |

Case ops whose case-mapped text equals the original return `Noop` without applying (no
spurious undo step or dirty mark — e.g. a selection of `中` or `🙂`). Both marginals
(`transpose_lines`, `delete_horizontal_space`) are kept per the ledger. Post-edit selection:
case ops re-select the transformed range; point edits collapse to a caret at the natural
landing position stated above.

**Binding:** all ten ship **default-bound in both presets** (that they get chords is the
decision; which chords is the plan's, conflict-checked against the CUA and WordStar tries).
Commands are keymap-agnostic; the three menu rows land in Format; Edit ⊆ palette holds
throughout.

### 3.4 Test intent (Phase 3)

- A9: menu test — Settings group row shows base `Wrap Column` with value `N…` tracking
  `view_opts.wrap_column`; `MenuMark::Text` arm covered in a `menu_leaf_parts` test; the
  existing registration/meta label assertions updated; law-2 test
  (`every_persisted_setting_has_a_command`) stays green with the unchanged id.
- A3b: registration meta tests pinning the new categories (filter in Format; if confirmed:
  transform in Format, the four deletes `menu: None`); the palette-exhaustive test is
  placement-insensitive and stays green.
- A14: per-command unit tests in `commands/textops.rs` (Arrange-Act-Assert): happy path,
  every Noop guard, one-undo-step, multibyte (`é`, `中`, `🙂`) for `transpose_chars` and the
  case ops, length-changing case mapping, selection-vs-word scope for case ops, and
  case-noop-equality. Registration test: ten ids present with the stated `menu` tags.

---

## Phase 4 — A8: the Documents dynamic menu section + contract amendment

### 4.1 Premise (verified)

`menu::grouped_commands` is purely static: it iterates `MENU_ORDER`, filters
`reg.commands()` by `meta.menu == Some(cat)`, and takes `&Editor` only to evaluate
`meta.state` for a row's value — it never adds or removes rows from live state. No
per-buffer registration or menu-population hook exists. A live "open documents" menu is
genuinely greenfield: this phase builds the seam.

### 4.2 The dynamic-menu-section mechanism

- **Row model.** `MenuView.groups` rows change from `(String, CommandId)` to
  `(String, MenuRowAction)` where

  ```rust
  pub enum MenuRowAction { Command(CommandId), SwitchBuffer(BufferId) }
  ```

  (`menu.rs`; exhaustive — the compiler forces every activation site to place new action
  kinds; this enum is the registration seam's dispatch half and where Effort-P action kinds
  will land). Static registry rows carry `Command(id)`; Documents rows carry
  `SwitchBuffer(id)`. Rows are **data, not commands**.
- **Provider seam.** A data-table row, NOT inline logic in `grouped_commands`
  (module-structure GATE — the hub stays closed to editing, open to new rows):

  ```rust
  pub struct DynamicSection {
      pub category: MenuCategory,
      pub rows: fn(&crate::editor::Editor) -> Vec<(String, MenuRowAction)>,
  }
  pub const DYNAMIC_SECTIONS: &[DynamicSection] =
      &[DynamicSection { category: MenuCategory::Documents, rows: crate::workspace::documents_menu_rows }];
  ```

  `grouped_commands` gains one generic step: after collecting a category's static rows,
  append the rows of any `DYNAMIC_SECTIONS` entry for that category (a category may be
  dynamic-only; the existing empty-category omission still applies to the combined result).
  Dynamic rows carry no chord/value columns — they enter `right_justify_leaves` as bare
  `(label, None, None, action)` entries.
- **The Documents provider.** `pub fn documents_menu_rows(editor: &Editor) -> Vec<(String,
  MenuRowAction)>` in `wordcartel/src/workspace.rs`: all buffers in **buffer-vec order (=
  open order — stable positions)**, **excluding scratch** (`is_scratch`), labeled by the
  existing `workspace::buffer_display_name` (so the dirty `*` prefix and `*untitled*` come
  for free), each `MenuRowAction::SwitchBuffer(id)`. The switcher palette stays **MRU** —
  the two orderings are deliberately complementary. Documents is never empty (an ordinary
  buffer always exists — `close_buffer_now` invariant).
- **Activation.** Both activation sites dispatch on the action:
  - keyboard — `menu::intercept`'s Enter arm (currently extracts a `CommandId`);
  - mouse — the menu branch of `mouse.rs::route_overlay` (currently extracts a `CommandId`
    via `chrome_geom::menu_dropdown_row_at`).
  `Command(id)` → `app::dispatch_overlay_command` exactly as today. `SwitchBuffer(id)` →
  close the menu, then resolve the id to an index and call `workspace::switch_to(editor,
  idx)`. This is NOT an out-of-registry bypass: `workspace::switch_to` is the **single shared
  buffer-activation transition** that the registered navigation commands ALSO route through —
  verified: the `switch_buffer` switcher's Buffers-row activation calls it from both keyboard
  (`palette.rs`, the `PaletteRow.buffer` handling) and mouse (`mouse.rs::route_overlay`
  palette branch), and `next_buffer`/`prev_buffer` (via `workspace::cycle`) and `goto_scratch`
  all call it too. So a `SwitchBuffer` row invokes the same mediated setter a command would
  (Law 6's "one setter" applied to navigation); only the ROW itself is data (not a registry
  command). Switching never closes a buffer, so the C4 dirty-close guard is untouched by
  construction — selecting a doc can never bypass it.
- **Type ripples** (mechanical, compiler-driven): `chrome_geom::menu_bar_layout`,
  `menu_dropdown_rect`, `menu_dropdown_row_at` signatures are typed on the groups' row tuple
  and follow the row-type change; `menu::empty_at` hydration and windowing
  (`list_window::keep_visible`, `windowed_indicator`) are index-based and unchanged.

### 4.3 Menu-home changes

- New top-level `MenuCategory::Documents` = the dynamic open-docs list ONLY. Final
  `MENU_ORDER = [File, Edit, Block, Format, View, Documents, Settings, Export]`.
- `next_buffer`, `prev_buffer`, `switch_buffer` move **out of the View menu** →
  `menu: None`: with direct selection in Documents they are duplicative on the menu surface,
  but remain live registry commands with their keymap bindings and palette rows
  (`switch_buffer` still opens the MRU switcher). `goto_scratch` and `toggle_scratch` stay
  in View.
- Scratch is excluded from Documents (A12 rider).

### 4.4 The contract amendment (a Phase-4 implementation task — NOT yet applied)

The governing contract is currently UNAMENDED: `docs/design/command-surface-contract.md` Law 1
still reads "nothing dispatches or mutates command-reachable state outside it," and its History
ends at 2026-07-07 — as it must at spec stage on `main` (nothing is implemented yet). Phase 4
of this effort WILL amend that file as a scheduled deliverable: the plan will edit the contract
to (i) add the "Dynamic menu sections" section below (placed after the shape rules), (ii) restate
Law 4 for dynamic rows, and (iii) append a 2026-07-10 History entry. The amendment is a merge
artifact of this effort, made only when Phase 4 lands — this spec DEFINES the text; it does not
assert the contract already carries it.

The History entry Phase 4 will append must record BOTH halves of the amendment: (a) dynamic-row
ACCOUNTING is exempt from laws 1/3/4 (a row need not be a registered command / palette entry),
and (b) a dynamic ACTION is still bound by law 1's mutation clause — it may only invoke a
shared setter a registered command also uses (Documents ↔ `switch_to`), never a novel path.
The exact History-entry text Phase 4 will add:

> - 2026-07-10 (A8 / command-surface curation): added the **Dynamic menu sections** section —
>   a menu category may carry rows generated from live editor state (data, not commands),
>   exempt from laws 1/3/4 for ROW ACCOUNTING only (a row need not be a registered command or
>   a palette entry); the row's ACTION remains bound by law 1's mutation clause and must invoke
>   a shared setter a registered command also uses (the Documents section ↔ `workspace::switch_to`,
>   shared with `switch_buffer`/`next_buffer`/`goto_scratch`). Restated law 4 as "every menu row
>   that NAMES A COMMAND is in the palette." Forward-compatible with Effort-P plugin menus.

The normative amendment text Phase 4 will insert into the contract:

> **Dynamic menu sections.** A dynamic menu section is a menu category whose rows are
> generated from live editor state at menu-build time by a registered provider —
> `fn(&Editor) -> Vec<(label, action)>` — rather than drawn from the command registry. Its
> rows are **data, not commands**: each row parameterizes an existing, validated state
> transition (e.g. switch-to-buffer) instead of naming a registry command.
> - **Exemption scope (row ACCOUNTING only).** Dynamic rows are exempt from law 4 (menu ⊆
>   palette) and from the registry-single-source reading of law 1 *for the rows themselves* —
>   a dynamic row need not correspond to a registered command and need not appear in the
>   palette. Laws 1 and 3 continue to govern the static registry unchanged; the
>   palette-exhaustiveness test remains defined over the static registry only. Law 4 is
>   restated as: every menu row that names a command is in the palette; dynamic-section rows
>   are data and exempt.
> - **Actions are NOT exempt from law 1's mutation clause — they must use a shared setter.**
>   Law 1 also forbids mutating command-reachable state *outside* the registry's setters. A
>   dynamic action does not escape that: it may ONLY invoke a **shared, mediated transition
>   that a registered command also routes through** (Law 6's "one setter" — e.g. Documents'
>   `SwitchBuffer` and the `switch_buffer`/`next_buffer`/`goto_scratch` commands all call the
>   single `workspace::switch_to`). A dynamic action MUST NOT introduce a novel mutation path
>   that no command uses; if a section needs a transition no command performs, add the command
>   (and its setter) first. This keeps every state change plugin-controllable and drift-free.
> - **Keyboard path preserved (law 5 still binds).** Every dynamic section must have a
>   registry-command keyboard equivalent covering the states its rows reach (Documents ↔
>   `switch_buffer`, which shares `switch_to` with the section's action).
> - **Registration seam.** Providers register via the `DYNAMIC_SECTIONS` data table — never
>   inline logic in the menu builder (module-structure GATE).
> - **Effort-P forward-compatibility.** This seam is the intended mechanism for
>   plugin-contributed menu sections (the second consumer). Plugins extend registration to
>   runtime without changing the rows-as-data model; plugin rows keep the same discipline —
>   parameterized dispatch of validated actions through the same activation seam, never raw
>   state mutation.

**Cross-check against `docs/design/effort-p-plugin-system-design-space.md`:** the seam matches
the P design direction on every axis it constrains — mediated access (the provider reads
`&Editor` and returns data; activation routes through `workspace::switch_to` / the registry,
never raw internals), registration-based extension (the eager-`plugin/` model registers
commands into the registry; menu sections register into `DYNAMIC_SECTIONS` the same way), and
no hot-path hooks (providers run only at menu-build time, an overlay-open cold path). The P
doc is silent on menus specifically, so this amendment defines the mechanism P will reuse;
the P brainstorm inherits `MenuRowAction` growth + runtime registration as its extension
points, not a new mechanism.

### 4.5 Test intent (Phase 4)

- `documents_menu_rows`: open order, scratch excluded, dirty `*` label, `SwitchBuffer` ids.
- `grouped_commands`: Documents group present via the seam; a category with only dynamic rows
  renders; static categories unchanged byte-for-byte (right-justify regression).
- Activation: keyboard Enter and mouse click on a Documents row switch the active buffer and
  close the menu; a `Command` row still dispatches through `dispatch_overlay_command`.
- Invariant gates: `palette_is_exhaustive_over_the_registry` stays green and UNCHANGED
  (defined over the static registry; dynamic rows never appear in the palette). A new law-4
  test pins the restated form: every `MenuRowAction::Command` id in the built menu resolves
  in the registry (and hence the palette).
- View-menu removal: `next_buffer`/`prev_buffer`/`switch_buffer` have `menu: None` but
  remain registered, bound, and palette-listed.

---

## Phase 5 — A13: mouse parity for the minibuffer + search overlays

Corrected scope (verified): `mouse.rs::route_overlay` already gives theme picker, file
browser, and outline scroll + click-to-select, and the prompt click-to-choose buttons via
`chrome_geom::prompt_choice_at`. The gap is the tail branch —
`if editor.minibuffer.is_some() || editor.search.is_some() {}` — zero mouse action. Both
overlays live on the bottom status row (`render.rs::paint_status`); the caret math to mirror
is in `render.rs::place_cursor`. Contract law 5 is already satisfied (full keyboard paths
exist) — this is pure mouse parity; no new keyboard surface, no new commands.

### 5.1 Minibuffer

- On `Down(Left)` on the status row (`area.height - 1`, the row `chrome_geom::menu_area`
  excludes) within the input line: set `Minibuffer::cursor` to the byte offset of the clicked
  character. Column → char-index mapping mirrors `place_cursor`'s convention exactly —
  `prompt.chars().count()` columns of prompt, then one column per char (the pre-existing
  char-count convention; wide-CJK width is deliberately NOT introduced — consistency with the
  caret painter over typographic correctness). Clicks past the text end clamp to
  `text.len()`; clicks on the prompt prefix, elsewhere on the row, or anywhere else in the
  frame are **consumed no-ops** — outside-click-to-dismiss is deliberately out of scope.
- Seam: a new `chrome_geom` hit-test helper (`minibuffer_click_byte(area, mb, col, row) ->
  Option<usize>`-shaped, beside the existing `*_row_at` helpers) + a real minibuffer branch
  in `route_overlay` replacing its half of the tail. All non-`Down(Left)` events remain
  consumed as today.

### 5.2 Search overlay

`SearchState` (`wordcartel/src/search_overlay.rs`): fields `field: Field`
(`Needle`/`Template`), `cursor` (byte, focused field), `needle`, `template`; match cache
`matches: Vec<Match>` (private; `matches()` accessor; `Match { start, end }`) with private
`cur_idx` (`set_current_at_or_after(off)` setter, `current()` accessor). The status-row bar
prefixes are exactly those `place_cursor` assumes: `"Find: "` before the needle;
`"Find: {needle}  Replace: "` before the template (Replace/Stepping phases).

- **Field click:** `Down(Left)` on the status row inside the needle text region → `field =
  Field::Needle`, `cursor` = clicked byte (same char-count mapping and end-clamp as 5.1);
  inside the template region (when the bar shows one) → `field = Field::Template`, likewise.
  Clicks elsewhere on the bar (the `format_search_bar` mode/case/count suffixes) are consumed
  no-ops. Seam: a `chrome_geom` helper returning `Option<(Field, usize)>`, sharing the prefix
  math with `place_cursor` (single source for the two prefix strings so painter and hit-test
  cannot drift).
- **Match click (strict step order — CACHE-ONLY refresh, then map, then choose):**
  1. **Cache-only refresh.** Snapshot the LIVE buffer and version from the active document
     (`let (rope, version) = { let d = &editor.active().document; (d.buffer.snapshot(),
     d.version) };`) and call `SearchState::recompute(&rope, version)` on `editor.search`
     DIRECTLY — NOT `search_ui::search_sync`. `recompute` is `pub fn recompute(&mut self, rope:
     &Rope, version: u64)` (`search_overlay.rs`) and does ONLY the version-keyed cache rebuild
     (no-op when `cache_sig = (needle, mode, case, version)` is unchanged). This must NOT go
     through `search_sync`, which — after `recompute` — also unfolds ancestors, sets the
     selection to the current match, `derive::rebuild`s and `ensure_visible`s (verified
     `search_ui.rs`): those side effects would move the viewport/selection BEFORE the click's
     screen coordinates are interpreted, mis-mapping the click. The refresh alone changes no
     viewport or selection state.
  2. **Map click → offset.** With the cache now current for the live buffer, map the event to a
     document byte via the existing body hit path (`editing_cell` → `CellHit::Text` →
     `nav::offset_at_cell`, the same functions the no-overlay click handler uses).
  3. **Choose + place.** If the mapped byte lies within some `Match { start, end }`
     (`start <= byte < end`) of the now-current match set (`matches()`): make it current —
     `set_current_at_or_after(start)` (exact, since match starts are unique and sorted) — then
     apply the identical placement tail `search_ui::search_step` uses
     (`registry::unfold_ancestors_of(editor, m.start)`; selection =
     `Selection::range(m.start, m.end)`; `derive::rebuild`; `nav::ensure_visible`). **The
     search overlay STAYS OPEN** — this mirrors next/prev stepping, option (a) of the ledger.
     Seam: extract the placement tail (steps after `recompute`) shared verbatim by `search_sync`
     and `search_step` into one `search_ui` helper the click path also calls in step 3.
  Clicks in the edit band NOT on a match, and all other events, remain consumed no-ops.
- **Staleness (why step 1 is cache-only, not `search_sync`).** The buffer CAN change under an
  open search overlay: `search_ui::intercept` returns `Handled::Pass` for non-key messages, so
  a background `Msg::FilterDone` falls through to normal handling and
  `jobs_apply.rs::apply_filter_done` applies the async filter result — mutating the buffer and
  calling `derive::rebuild` — while leaving `editor.search` open. A raw read of the cached
  `matches()` after such an edit could map a click onto stale offsets. The version-keyed
  `recompute` (keyed on `(needle, mode, case, version)`) is exactly the guard, and calling it
  by itself — the cache-only step 1 — refreshes the match set without the viewport/selection
  side effects `search_sync` bundles. Order matters: recompute FIRST (so `matches()` reflects
  the live buffer), map SECOND, choose THIRD.

### 5.3 Test intent (Phase 5)

Unit tests in `mouse.rs` (existing pattern: constructed `MouseEvent` + editor):

- Minibuffer: mid-text click sets the exact byte (multibyte `é`/`中` text — char-column
  convention); past-end click clamps; prompt-region and edit-band clicks change nothing and
  are consumed (minibuffer stays open, no caret/selection change behind it).
- Search: needle click focuses Needle + positions cursor; template click in Replace phase
  focuses Template; suffix-region click is a no-op; clicking a highlighted match sets it
  current (selection = the match range, `current_ordinal` reflects it) with
  `editor.search.is_some()` after; a non-match body click changes neither `cur_idx` nor the
  selection.
- Search stale-cache regression (cache-only refresh): with search open, mutate the buffer
  (apply an edit that bumps `document.version` and shifts match offsets) WITHOUT the click
  path having re-synced, then a match-click asserts:
  - The step-1 cache-only `recompute` causes NO viewport scroll and NO change to the
    editing/`document.selection` — those side effects belong only to `search_sync`, which the
    click path must NOT call. (`cur_idx` is deliberately NOT pinned across the refresh:
    `recompute` on a version change clears it to `None` and resets it to
    `first_at_or_after(self.origin)` per `search_overlay.rs`, so asserting it "unchanged" would
    contradict real code.)
  - After the FULL three-step click on a byte inside a real current-buffer match, the selected
    match is the CLICKED one — `document.selection == Selection::range(m.start, m.end)` for the
    match under the click and `cur_idx`/`current()` reflect it (set solely by step 3's
    `set_current_at_or_after`, mapped against fresh — not stale — offsets). This proves the
    cache was current for the live buffer version before the click was mapped (a click after an
    async `apply_filter_done` edit maps against fresh offsets), and `editor.search.is_some()`
    holds after (overlay stays open).
  - Control: a click whose mapped byte is NOT inside any match performs no step-3 placement;
    since `recompute` touches only `SearchState`'s own cache fields (never `document.selection`
    or the viewport), the editing selection is unchanged from before the click and the overlay
    stays open.
- Regression: the three already-clickable overlays' tests untouched and green.

---

## Command-surface contract conformance (per the contract's own requirement)

- **Law 1 — registry is the single source of truth.** All 12 new commands
  (`select_marked_block`, `toggle_scratch`, ten A14 ops) are `builtins()` registrations
  dispatching through `Handler`; no state mutation is reachable outside them. Phase 4's
  dynamic rows are the one deliberate exception the Phase-4 contract amendment (§4.4) WILL
  authorize, and only for row ACCOUNTING (a row need not be a registered command): the ACTION
  a Documents row triggers, `MenuRowAction::SwitchBuffer` → `workspace::switch_to`, is NOT a
  bypass of law 1's mutation clause — `switch_to` is the single shared buffer-activation setter
  that the registered `switch_buffer`/`next_buffer`/`prev_buffer`/`goto_scratch` commands also
  route through (verified: `palette.rs`, `mouse.rs::route_overlay`, `workspace::cycle`,
  `workspace::goto_scratch` all call it). So the dynamic action reuses a command's mediated
  setter (Law 6's one-setter); only the row's registry membership is exempt, once Phase 4
  applies the amendment. Law 1 over the static registry is otherwise unchanged, and the
  contract file itself is only edited when Phase 4 lands (not by this spec).
- **Law 2 — every user-settable option is a command.** No new persisted setting is
  introduced (`scratch_return` is transient runtime state, not a `SettingsSnapshot` field);
  `set_wrap_column` keeps its id. The `every_persisted_setting_has_a_command` guard runs
  unchanged as a merge gate.
- **Law 3 — palette exhaustive.** All new commands appear in the palette automatically
  (registry-driven rows). Dynamic Documents rows are NOT palette rows (data-not-commands,
  per the amendment); `palette_is_exhaustive_over_the_registry` remains defined over the
  static registry, byte-identical, and is a merge gate.
- **Law 4 — menu ⊆ palette.** Holds for every command row (Block, Format, View, Settings
  moves included — a `meta.menu` edit can never break it). Restated by the amendment for
  dynamic rows (exempt as data); the new law-4 test (4.5) pins the command-row form.
- **Law 5 — every mouse affordance has a keyboard path.** Phase 5 adds mouse paths to
  keyboard-complete overlays (parity direction that needs no new work to conform). The
  Documents menu's mouse/keyboard rows are covered by `switch_buffer` + the palette switcher.
- **Law 6 — one setter per option.** `set_wrap_column`'s single mutation path
  (`wrap_column_submit`) is untouched; A9 adds a read-only state fn. No profile/preset
  changes in this effort.
- **Law 7 — hints track the active keymap.** New commands get hints via the same
  `keymap.chord_for(id)` path in menu and palette; nothing bypasses it. The
  `hints_reresolve_on_preset_switch` + `custom_bind_surfaces_in_menu_and_palette` tests run
  as merge gates; the plan's default chords for the ten A14 ops + `toggle_scratch` are
  preset-trie entries, so re-resolution covers them automatically.
- **Rule 8 — multi-state = set-per-state + stateful representative.** N/A to the new
  commands (none is a multi-state option). A9 follows the stateful-representative *display*
  mechanism without being a cycle — its menu row is the option's single command, which
  rule 8 does not forbid (the rule constrains multi-state options; wrap column is
  prompt-set, not enumerable).
- **Rule 9 — presets never the only door.** No preset/profile touched.
- **Rule 10 — commands are the plugin spine; nullary today.** All new commands are nullary.
  The one parameterized action (`SwitchBuffer(BufferId)`) is deliberately NOT a command —
  the amendment keeps the nullary-command rule intact while giving Effort P the
  rows-as-data mechanism; P may later collapse set-value families into parameterized
  commands per rule 10's existing note.
- **Amendment:** Phase 4 amends the contract (dynamic menu sections) as a deliberate act
  with a History entry — specified in 4.4, cross-checked against the Effort-P design space.

**Merge-gate invariant tests exercised by this effort:** `palette_is_exhaustive_over_the_registry`,
`every_persisted_setting_has_a_command`, `hints_reresolve_on_preset_switch`,
`custom_bind_surfaces_in_menu_and_palette`, plus the standing gates (workspace clippy clean,
`clippy::too_many_lines`, `wordcartel/tests/module_budgets.rs` hub budgets — `builtins()`
keeps its existing item-local `#[allow(clippy::too_many_lines)]` as a flat data table).

---

## Binding policy (chords deferred to the plan)

Commands are keymap-agnostic; palette exhaustiveness makes every command keyboard-reachable
even unbound. Per the ledger:

- **Default-bound in both presets:** the ten A14 atomic edits + `toggle_scratch`. That they
  get default chords is decided here (atomic edits would otherwise ship ergonomically
  inert); WHICH chords is the plan's job, conflict-checked per preset (CUA Ctrl-based;
  WordStar `^K`-prefix trie).
- **Palette-first, unbound by default (user-bindable):** `select_marked_block`. The WordStar
  preset MAY still chord the block ops — the plan decides.
- **Unchanged:** `filter`, `set_wrap_column`, and every existing binding.

---

## Effort-wide consequences

- **Menu bar grows 6 → 8 categories** (Block, Documents). `chrome_geom::menu_bar_layout_cats`
  lays labels left-to-right at `label chars + 2` columns each with NO horizontal windowing:
  the bar grows from 44 columns (File 6 + Edit 6 + Format 8 + View 6 + Settings 10 +
  Export 8) to **62 columns** (+ Block 7, + Documents 11). The existing "menu windowing" is
  the dropdown's VERTICAL windowing (`list_window`), not bar-width handling — on terminals
  narrower than 62 columns the trailing categories (Settings, Export) render clipped and
  become mouse-unreachable (keyboard Left/Right still cycles to them via `menu::intercept`).
  **Verification required before merge:** exercise the 8-category bar at the smallest
  supported terminal size (the tiny-terminal guard / smoke suite) and confirm the clipping
  behavior is acceptable there; if not, bar overflow handling is a surfaced follow-up item,
  not silent scope growth in this effort.
- **Registry grows ~156 → ~168 commands** (+12); menu-listed rows shift per Phases 2–4
  (+14-row Block group, +3 Format case ops, +`toggle_scratch`; −3 View buffer-nav rows;
  −4 Edit delete rows → palette-only, and `transform` View→Format — both confirmed at spec review).
  The palette-exhaustive and registration-order tests absorb this mechanically.
- **Test-suite ripples** (updated, not weakened): scratch-in-rotation tests (2.3), the menu
  grouping/label tests (Block/Documents/wrap-column value), registry meta/label assertions,
  and the e2e journeys that traverse the View menu or the switcher.

## Flagged items — RESOLVED at spec review (2026-07-10)

All five were surfaced (not silently resolved) and confirmed by the human at spec review:

1. **A9 / `MenuMark`:** the ledger's "`MenuMark::Value`" cannot carry a runtime-formatted
   value (`Value(&'static str)`); the spec adds `MenuMark::Text(String)` and drops `Copy`
   (3.1). Intent preserved; carrier extended. **CONFIRMED — accepted.**
2. **A3b sweep judgment call 1:** `transform` View → Format (3.2). **CONFIRMED — move.**
3. **A3b sweep judgment call 2:** `delete_word_back` / `delete_word_forward` / `delete_line`
   / `delete_to_line_end` Edit → palette-only (3.2). **CONFIRMED — move.**
4. **Filter shell = POSIX `sh -c`**, not `$SHELL -c` (1.3) — the ledger listed both forms;
   the existing `run_subprocess` mechanism and determinism argue for `sh`. **CONFIRMED — `sh -c`.**
5. **`goto_scratch` also records the return buffer** (2.2) — a coherence extension of the
   ledger's "toggle remembers prior buffer" so goto→toggle round-trips. **CONFIRMED — accepted.**

Menu-bar width (§Effort-wide consequences): the 62-column 8-category bar clip below 62 cols is
accepted as a **verify-before-merge** item; if the clip is unacceptable at the smallest supported
size, bar-overflow handling is a surfaced follow-up item, not in-effort scope growth.
