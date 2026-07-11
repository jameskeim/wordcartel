# Command-Surface Curation — Implementation Plan

**Date:** 2026-07-10
**Spec:** `docs/superpowers/specs/2026-07-10-command-surface-curation-design.md` (Codex-GO).
**Branch:** `effort-command-surface-curation`.
**Discipline:** one effort, phased. Each task below is independently dispatchable to a fresh
implementer subagent; TDD (failing test → impl → green → commit). Anchor on symbol NAMES; the
tree is settled. Do NOT run `cargo fmt`. Match house style (dense, hand-wrapped, em-dash `—`).

All symbols/signatures below are verified against the real source. Key templates:
- Command registration: `Registry::register(id, label, menu, handler)` and
  `Registry::register_stateful(id, label, menu, state_fn, handler)` (`registry.rs`).
  `Handler = fn(&mut Ctx) -> CommandResult`; `Ctx { editor, clock, executor, msg_tx }`.
- Atomic edit template (`commands/edit.rs`): compute `(from, to)` + replacement text; early
  `CommandResult::Noop`; `let (cs, edit) = commands::build_range_replace(from, to, &text,
  doc_len);` (or `ChangeSet::delete` for pure deletes); `let txn =
  Transaction::new(cs).with_selection(Selection::single(pos));`
  `editor.apply(txn, edit, EditKind::Other, clock);` then `settle_after_edit(editor)`.
  Verified `Editor::apply(&mut self, txn: Transaction, edit: block_tree::Edit, kind: EditKind,
  clock: &dyn Clock)` and `pub fn build_range_replace(from,to,text,doc_len) -> (ChangeSet, Edit)`.
- Word-at-caret scope: `commands::scope_range_at(editor, head, commands::Scope::Word) ->
  (usize, usize)` (whitespace → nearest word; both `pub`).
- Buffer transition: `workspace::switch_to(editor, idx)` (the ONE shared setter —
  `palette.rs`, `mouse.rs::route_overlay`, `workspace::cycle`, `workspace::goto_scratch` all
  call it).

**GLOBAL REGISTRATION-ORDER RULE (Codex F4 — applies to Tasks 1.1, 2.2, 3.3).** `save_settings`
is CURRENTLY the last `r.register(...)` row in `registry.rs::builtins` (verified — it is the
final register call; the in-file comment above it and `e2e.rs::journey_palette_end_reaches_last_command`
both depend on it staying last: End+Enter in the palette must dispatch `save_settings`). Therefore
EVERY new registry row this effort adds — `select_marked_block` (1.1), `toggle_scratch` (2.2),
and the ten `textops` commands (3.3) — MUST be registered BEFORE the `save_settings` row (place
them with their sibling families earlier in `builtins`, never appended after `save_settings`).
This preserves both the registration-order invariant (`commands_iterate_in_registration_order_with_meta`)
and the e2e last-command journey. The palette-exhaustive test counts rows regardless of position,
so it is unaffected.

---

## Command-surface contract conformance (plan level)

Per the contract's requirement that the plan (not only the spec) state conformance:

- **Law 1 (registry single source).** Every new command (`select_marked_block`,
  `toggle_scratch`, ten A14 ops) is a `builtins()` registration. The A8 dynamic-row exception
  is authorized ONLY by the Phase-4 contract amendment (Task 4.3), and only for row
  ACCOUNTING; the row ACTION (`SwitchBuffer` → `workspace::switch_to`) reuses the same shared
  setter registered commands use — not a novel mutation path.
- **Law 2 (every option a command).** No new persisted `SettingsSnapshot` field is added
  (`scratch_return` is transient runtime state). `set_wrap_column` keeps its id. GATE
  `every_persisted_setting_has_a_command` (`settings.rs`) stays green.
- **Law 3 (palette exhaustive).** All new commands are registry rows → auto-appear in the
  palette. Dynamic Documents rows are NOT palette rows (data). GATE
  `palette_is_exhaustive_over_the_registry` (`palette.rs`) stays defined over the static
  registry, unchanged.
- **Law 4 (menu ⊆ palette).** Holds for every command row. Restated by the Task-4.3 amendment
  for dynamic rows (exempt as data).
- **Law 5 (mouse ⇒ keyboard).** Phase 5 adds mouse paths to keyboard-complete overlays;
  Documents rows share `switch_buffer`'s keyboard path.
- **Law 6 (one setter).** `set_wrap_column`'s setter (`prompts::wrap_column_submit`)
  unchanged; A9 adds a read-only state fn. `SwitchBuffer` uses `switch_to` (shared).
- **Law 7 (hints track keymap).** New chords are preset-trie entries; menu/palette hints
  resolve via `keymap.chord_for(id)`. GATEs `hints_reresolve_on_preset_switch` (`keymap.rs`),
  `custom_bind_surfaces_in_menu_and_palette` (`menu.rs`) stay green.
- **Rule 8/9/10.** New commands are nullary; the one parameterized action (`SwitchBuffer`) is
  deliberately data, not a command (keeps Rule 10's nullary-command rule intact).

**Merge GATEs every phase must keep green:** `cargo test` (all suites); `cargo clippy
--workspace --all-targets` clean (deny); `clippy::too_many_lines` (threshold 100) +
`wordcartel/tests/module_budgets.rs` (app.rs ≤1000, render.rs ≤900, timers.rs ≤400 — none of
which this effort should grow materially); the four invariant tests above; and
`both_presets_resolve_against_builtins` (`keymap.rs`) once bindings land.

---

# PHASE 1 — A11: scope convention, `select_marked_block`, filter → shell

## Task 1.1 — `select_marked_block` command

**Failing test:** `blocks_marked.rs` `#[cfg(test)]` →
`fn select_marked_block_selects_range_and_keeps_block()`: build an `Editor` with
`e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 5, hidden: false })` over text
`"hello world\n"`; call `select_marked_block(&mut e)`; assert
`e.active().document.selection.primary()` has `.from()==0 && .to()==5`, and
`e.active().marked_block.is_some()` (block survives). Second test
`fn select_marked_block_no_block_sets_status()`: no marked block → selection unchanged and
`e.status` non-empty.

**Implementation:**
- `blocks_marked.rs`: add
  ```rust
  /// Select the marked block's range (the block → selection bridge, A11.3). The marked
  /// block is a target, not implicit scope — this makes it the active selection so the
  /// universal selection-primary convention then governs filter/transform/case ops.
  pub fn select_marked_block(editor: &mut Editor) {
      match editor.active().marked_block {
          Some(MarkedBlock { start, end, .. }) => {
              editor.active_mut().document.selection =
                  wordcartel_core::selection::Selection::range(start, end);
              crate::derive::rebuild(editor);
              crate::nav::ensure_visible(editor);
          }
          None => { editor.status = "no marked block".into(); }
      }
  }
  ```
  (`MarkedBlock` is already imported in `blocks_marked.rs`; `Selection::range(a, b)` verified in use across `search_ui.rs`/`commands.rs`.)
- `registry.rs::builtins`: add ONE row near the block family (which registers well before the
  final `save_settings` row — global registration-order rule, Codex F4). **Register with
  `menu: None` in this task** (the `Block` category does not exist until Task 2.1, which will
  flip it to `Some(MenuCategory::Block)`):
  ```rust
  r.register("select_marked_block", "Select Block", None,
      |c| { crate::blocks_marked::select_marked_block(c.editor); CommandResult::Handled });
  ```

**Green.** **Commit:** `feat(blocks): add select_marked_block bridge command (A11.3)`.

## Task 1.2 — filter runs through a shell (`sh -c`)

**Failing test:** `prompts.rs` `#[cfg(test)]` →
`fn submit_filter_line_uses_shell_single_argv()`: call `submit_filter_line(&mut e, "sed
's/a  b/c/'", &tx)`; capture the `FilterSpec` (refactor `submit_filter_line` to build the spec
via a small testable helper, OR assert via a seam — simplest: extract
`fn build_filter_spec(line: &str) -> Option<FilterSpec>` returning `None` on trimmed-empty, and
test it): assert `spec.shell == true`, `spec.argv == vec!["sed 's/a  b/c/'".to_string()]` (the
line verbatim, single element — NOT whitespace-split), `spec.input` is
`Input::SelectionElseBuffer`, `spec.timeout == Duration::from_secs(10)`, `spec.max_output ==
crate::limits::MAX_FILTER_OUTPUT`. Second test `fn build_filter_spec_trimmed_empty_is_none()`:
`"   "` → `None`.

**Implementation:**
- `prompts.rs::submit_filter_line`: replace the whitespace-split argv with the single-element
  verbatim line + `shell: true`, and the empty guard with `line.trim().is_empty()`:
  ```rust
  pub(crate) fn submit_filter_line(editor: &mut Editor, line: &str,
      msg_tx: &std::sync::mpsc::Sender<Msg>) {
      let Some(spec) = build_filter_spec(line) else {
          editor.status = "filter: no command given".into();
          return;
      };
      crate::filter::dispatch_filter(editor, spec, msg_tx.clone());
  }
  /// Build the FilterSpec for an interactive filter line. The line is passed to the
  /// shell VERBATIM as a single argv element (`run_subprocess` joins argv for `sh -c`),
  /// so quoting/pipes/redirects survive — splitting+rejoining would collapse quoted
  /// whitespace. Trust boundary: user-typed at an interactive prompt (vi `!`), distinct
  /// from the untrusted `submit_transaction` path. Caps (timeout/max_output) + the
  /// dispatch_filter panic isolation are kept unchanged.
  fn build_filter_spec(line: &str) -> Option<crate::filter::FilterSpec> {
      if line.trim().is_empty() { return None; }
      Some(crate::filter::FilterSpec {
          argv: vec![line.to_string()],
          shell: true,
          disposition: crate::filter::Disposition::Filter,
          input: crate::filter::Input::SelectionElseBuffer,
          timeout: std::time::Duration::from_secs(10),
          max_output: crate::limits::MAX_FILTER_OUTPUT,
      })
  }
  ```
  (`FilterSpec { argv, shell, disposition, input, timeout, max_output }` verified in `filter.rs`;
  `run_subprocess` builds `vec!["sh","-c",argv.join(" ")]` when `shell` — verified.)
- `registry.rs`: change the `filter` command's minibuffer prompt from `"> "` to `"sh> "`
  (signals shell semantics): in the `filter` registration,
  `c.editor.open_minibuffer("sh> ", crate::minibuffer::MinibufferKind::Filter)`.
- Update the `Minibuffer::default`/test in `minibuffer.rs` if it asserts the `"> "` Filter
  prompt (the `Minibuffer { prompt: "> ".into(), … MinibufferKind::Filter }` test builder at
  the bottom of `minibuffer.rs` — leave that literal; it does not assert the command's prompt).

**Integration test (real subprocess, existing filter-test style):** add to the `filter.rs`
suite `fn shell_pipeline_survives_quoted_whitespace()` — a `FilterSpec { argv: vec!["tr a-z
A-Z".into()], shell: true, … }` over input `"ab\n"` yields `"AB\n"`; and a quoted-double-space
`sed` program passes through verbatim. (Existing timeout/max_output/panic tests remain green
with `shell: false`.)

**Green.** **Commit:** `feat(filter): run interactive filter line through sh -c (A11.4)`.

## Task 1.3 — Filter prompt example hint (ghost)

**Failing test:** `render.rs` `#[cfg(test)]` →
`fn filter_minibuffer_shows_example_hint_when_empty()`: open the Filter minibuffer with empty
text on a `TestBackend`; render; assert the status row contains a substring of
`FILTER_EXAMPLE_HINT` (e.g. `"sort"`). Second test
`fn filter_hint_gone_once_text_typed()`: with `mb.text = "x"`, the hint substring is absent.
(Mirror `renders_active_minibuffer_on_status_row`.)

**Implementation:**
- `minibuffer.rs`: add `pub(crate) const FILTER_EXAMPLE_HINT: &str = "  e.g. sort | uniq · fmt
  -w 72 · sed s/a/b/g · tr a-z A-Z · column -t";`
- `render.rs::paint_status`: in the `editor.minibuffer` arm, when
  `mb.kind == crate::minibuffer::MinibufferKind::Filter && mb.text.is_empty()`, append
  `crate::minibuffer::FILTER_EXAMPLE_HINT` to the composed status text AFTER the caret column
  (the caret sits at `prompt + text`, so the trailing hint renders past it). Keep it within the
  `status_style` span (dim reads acceptably); do NOT add a second styled span (keeps the hub
  thin — render.rs budget 900). Guard: the hint is only in the minibuffer branch, so it never
  affects the normal status row. Keep the total added lines minimal (≤5).

**Green.** **Commit:** `feat(filter): ghost example hint on the empty Filter prompt (A11.4)`.

---

# PHASE 2 — A10 (Block menu) + A12 (scratch round-trip)

## Task 2.1 — `MenuCategory::Block` + move the block family

**Failing test:** `menu.rs` `#[cfg(test)]` →
`fn block_category_groups_the_block_family()`: build the menu; assert a `Block` group exists,
appears edit-adjacent (index after `Edit`, before `Format` in the built top-level order), and
contains `block_begin`, `block_write`, `copy_block_to_scratch`, `select_marked_block` (spot
check); assert `block_write` is NOT in the `File` group. Also update
`registry.rs` meta tests that pin the old categories (any asserting `block_*`/scratch verbs are
`Edit`, or `block_write` is `File`).

**Implementation:**
- `registry.rs`:
  - `MenuCategory`: add `Block` →
    `pub enum MenuCategory { File, Edit, Block, Format, View, Settings, Export }`.
  - `MENU_ORDER`: **7 entries this phase** (Documents lands in Task 4.2):
    ```rust
    pub const MENU_ORDER: [MenuCategory; 7] = [MenuCategory::File, MenuCategory::Edit,
        MenuCategory::Block, MenuCategory::Format, MenuCategory::View, MenuCategory::Settings,
        MenuCategory::Export];
    ```
  - Flip `meta.menu` to `Some(MenuCategory::Block)` on the existing rows: `block_begin`,
    `block_end`, `mark_block_from_selection`, `block_copy`, `block_move`, `block_delete`,
    `block_jump_begin`, `block_jump_end`, `block_toggle_hidden`, `block_clear` (were `Edit`);
    `block_write` (was `File`); `copy_block_to_scratch`, `move_block_to_scratch` (were `Edit`);
    and `select_marked_block` (was `None` from Task 1.1) → `Some(MenuCategory::Block)`.
- `menu.rs::category_label` (exhaustive match — the compiler forces this): add
  `MenuCategory::Block => "Block"`.

**Green.** **Commit:** `feat(menu): add Block category; move the marked-block family (A10)`.

## Task 2.2 — `toggle_scratch` + scratch excluded from rotation

**Failing tests (`workspace.rs`):**
- `fn toggle_scratch_round_trips_to_prior_buffer()`: two ordinary buffers, active = buf B;
  `toggle_scratch` → active is scratch; `toggle_scratch` again → active is B.
- `fn toggle_scratch_from_closed_prior_falls_back_to_mru()`: enter scratch recording B, then
  simulate B closed (remove from buffers/mru), `toggle_scratch` → lands on the most-recent
  live ordinary buffer (`editor.mru` FORWARD, first non-scratch), not scratch.
- Rewrite `cycle_wraps_in_stable_order_including_scratch` → `cycle_skips_scratch()`: in a
  `[A, scratch, B]` layout, from scratch `next_buffer` → `B` (the buffer FOLLOWING scratch) and
  `prev_buffer` → `A` (the buffer PRECEDING scratch); from an ordinary buffer, neither ever
  lands on scratch.
- Update `switcher_rows_mru_order_with_display_names`: `buffer_switch_rows` no longer contains
  the `*scratch*` row.
- `fn goto_scratch_records_return_for_toggle()`: `goto_scratch` from B then `toggle_scratch`
  returns to B (shared return-recording).

**Implementation:**
- `editor.rs`: add field `pub scratch_return: Option<BufferId>` beside `scratch_id`/`mru`
  (init `None` in the `Editor` constructor(s)).
- `workspace.rs`:
  - Add a private helper used by both scratch entries:
    ```rust
    fn enter_scratch(editor: &mut Editor) {
        let cur = editor.active().id;
        if !editor.is_scratch(cur) { editor.scratch_return = Some(cur); }
        if let Some(sid) = editor.scratch_id {
            if let Some(idx) = editor.buffers.iter().position(|b| b.id == sid) {
                switch_to(editor, idx);
            }
        }
    }
    ```
  - `goto_scratch`: delegate to `enter_scratch(editor)` (records the return buffer; behavior
    otherwise unchanged — still always goes to scratch).
  - Add:
    ```rust
    /// Round-trip to/from the scratch buffer (A12). Not on scratch → record + go to scratch.
    /// On scratch → return to the recorded buffer if it still resolves, else the MRU-front
    /// ordinary buffer; else stay with a hint.
    pub fn toggle_scratch(editor: &mut Editor) {
        if editor.is_scratch(editor.active().id) {
            // MRU is most-recent-FIRST (Editor::touch_mru does mru.insert(0, id); Codex F1),
            // so iterate FORWARD (no .rev()) and take the first live, non-scratch id.
            let target = editor.scratch_return
                .filter(|id| editor.by_id(*id).is_some())
                .or_else(|| editor.mru.iter().copied()
                    .find(|id| !editor.is_scratch(*id) && editor.by_id(*id).is_some())
                    .or_else(|| editor.buffers.iter().map(|b| b.id)
                        .find(|id| !editor.is_scratch(*id))));
            match target.and_then(|id| editor.buffers.iter().position(|b| b.id == id)) {
                Some(idx) => switch_to(editor, idx),
                None => editor.status = "no other buffer".into(),
            }
        } else {
            enter_scratch(editor);
        }
    }
    ```
    (MRU direction verified against `editor.rs::touch_mru` — `self.mru.insert(0, id)`, so
    `editor.mru[0]` is most-recent — and `workspace.rs::buffer_switch_rows`, which iterates
    `editor.mru` forward under the doc "most-recent first". Forward iteration is the fallback.)
  - `cycle`: skip scratch, anchoring on the ACTIVE index (which IS scratch's real
    `editor.buffers` index when active is scratch, since `install_scratch` pushes scratch into
    `buffers`) and stepping in the requested direction to the nearest non-scratch buffer — the
    spec's "following (next) / preceding (prev) scratch's position" rule (Codex F2). Do NOT
    build an ordinary-only ring and `unwrap_or(0)` — that loses scratch's position (for
    `[A, scratch, B]`, `prev` from scratch would wrongly return `B` instead of `A`):
    ```rust
    fn cycle(editor: &mut Editor, delta: isize) {
        let n = editor.buffers.len();
        if n == 0 { return; }
        // Need at least two ordinary (non-scratch) buffers to rotate between.
        let ordinary = editor.buffers.iter().filter(|b| !editor.is_scratch(b.id)).count();
        if ordinary <= 1 { return; }
        let start = editor.active as isize;   // == scratch's index when active is scratch
        let step = if delta >= 0 { 1 } else { -1 };
        let mut i = start;
        loop {
            i = (i + step).rem_euclid(n as isize);
            let idx = i as usize;
            if !editor.is_scratch(editor.buffers[idx].id) { switch_to(editor, idx); return; }
            if i == start { return; } // full loop with no ordinary landing (guarded above)
        }
    }
    ```
    Test both directions from scratch in a `[A, scratch, B]` layout: `next_buffer` → `B`
    (following scratch), `prev_buffer` → `A` (preceding scratch). And from an ordinary buffer,
    scratch is never a destination.
  - `buffer_switch_rows`: filter out scratch —
    `for &id in &editor.mru { if editor.by_id(id).is_some() && !editor.is_scratch(id) { … } }`
    and likewise the buffer-vec append loop skips `is_scratch`.
- `registry.rs::builtins`: register `toggle_scratch` in the **View** menu, immediately after
  `goto_scratch` (adjacency = registration order; `goto_scratch` sits well before
  `save_settings`, so the global registration-order rule (Codex F4) is honored):
  ```rust
  r.register("toggle_scratch", "Toggle Scratch Buffer", Some(MenuCategory::View),
      |c| { crate::workspace::toggle_scratch(c.editor); CommandResult::Handled });
  ```
  (Chord assigned in Task 3.4.)

**Green.** **Commit:** `feat(workspace): toggle_scratch round-trip; exclude scratch from rotation (A12)`.

---

# PHASE 3 — A9, A3b, A14, default bindings

## Task 3.1 — A9: `set_wrap_column` becomes a stateful menu row (`MenuMark::Text`)

**Failing tests:**
- `registry.rs` `fn set_wrap_column_is_stateful_with_value_label()`: get
  `reg.meta(CommandId("set_wrap_column"))`; assert `meta.state.is_some()`; with
  `ed.view_opts.wrap_column = 80`, `(meta.state.unwrap())(&ed) == MenuMark::Text("80…".into())`.
- `menu.rs` `fn wrap_column_row_shows_value()`: built Settings group has a row whose base is
  `Wrap Column` and value contains `80` + the ellipsis.

**Implementation:**
- `registry.rs`:
  - `MenuMark`: add the owned variant and **drop `Copy`** (keep the rest):
    ```rust
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum MenuMark { OnOff(bool), Value(&'static str), Text(String) }
    ```
    Ripple audit (verified — exactly these): the ONLY match site is `menu.rs::menu_leaf_parts`
    (add arm below). All other uses are constructors (`MenuMark::OnOff(..)`/`MenuMark::Value(..)`
    across `registry.rs`) — unaffected. `CommandMeta` keeps its own `#[derive(Clone, Copy)]`:
    its `state` field is a `fn` pointer (Copy) that RETURNS a `MenuMark`; it never stores one, so
    dropping `MenuMark: Copy` does not touch `CommandMeta`'s derive. Registry tests using
    `matches!(f(&ed), MenuMark::Value("Zen"))` / `assert_eq!(…, MenuMark::OnOff(true))` compile
    unchanged (consume by value; `PartialEq` kept).
  - Convert `set_wrap_column` from `register` to `register_stateful`, label
    `"Wrap Column: Set…"`, state fn returns the live value:
    ```rust
    r.register_stateful("set_wrap_column", "Wrap Column: Set\u{2026}", Some(MenuCategory::Settings),
        |e| crate::registry::MenuMark::Text(format!("{}\u{2026}", e.view_opts.wrap_column)),
        |c| { c.editor.open_minibuffer("Wrap column: ", crate::minibuffer::MinibufferKind::WrapColumn);
              CommandResult::Handled });
    ```
    (A non-capturing closure coerces to `fn(&Editor) -> MenuMark`. `menu_leaf_parts` splits the
    label at `:` → base `Wrap Column`; the palette shows the verbatim label `Wrap Column: Set…`.
    The setter/minibuffer flow — `MinibufferKind::WrapColumn` → `wrap_column_submit` — is
    unchanged, honoring Law 6.)
- `menu.rs::menu_leaf_parts`: add the arm
  ```rust
  MenuMark::Text(s) => s,
  ```
  (returns the owned `String` directly).

**Green.** **Commit:** `feat(view): show live Wrap Column value in the menu; add MenuMark::Text (A9)`.

## Task 3.2 — A3b: placement sweep (`meta.menu` edits)

**FLAGGED — two of these are spec judgment calls (spec §3.2, still human-confirm; see the
Flags section). Implement the spec's recommended end state; the reviewer/human may revert.**

**Failing test:** `registry.rs` `fn a3b_placement_sweep_categories()`: assert
`reg.meta(CommandId("filter")).menu == Some(MenuCategory::Format)`;
`reg.meta(CommandId("transform")).menu == Some(MenuCategory::Format)`; and
`reg.meta(CommandId(id)).menu == None` for each of `delete_word_back`, `delete_word_forward`,
`delete_line`, `delete_to_line_end`.

**Implementation (`registry.rs` `meta.menu` edits only):**
- `filter`: `Some(MenuCategory::Edit)` → `Some(MenuCategory::Format)` (normative — spec §3.2).
- `transform`: `Some(MenuCategory::View)` → `Some(MenuCategory::Format)` (**FLAG 1**).
- `delete_word_back`, `delete_word_forward`, `delete_line`, `delete_to_line_end`:
  `Some(MenuCategory::Edit)` → `None` (**FLAG 2**; default chords untouched — placement only).
- Update any registry meta test that pinned these old categories (e.g. a test asserting
  `filter`/`transform` category, or the four deletes in Edit).

**Green.** **Commit:** `refactor(menu): A3b placement sweep — filter/transform→Format, keystroke deletes palette-only`.

## Task 3.3 — A14: ten atomic-edit commands (`commands/textops.rs`)

**Failing tests:** new `commands/textops.rs` `#[cfg(test)]`, one per command, Arrange-Act-Assert
on a `Editor::new_from_text`. Names + core assertions:
- `fn transpose_chars_swaps_around_caret()`; `fn transpose_chars_multibyte()` (`é中` safe);
  `fn transpose_chars_noop_at_edges()`.
- `fn transpose_words_swaps_words_keeping_gap()`; `fn transpose_words_noop_without_two()`.
- `fn transpose_lines_swaps_with_line_above()`; `fn transpose_lines_noop_on_first_line()`.
- `fn upcase_word_at_caret_or_selection()`; `fn downcase_…`; `fn capitalize_…`;
  `fn case_op_noop_when_unchanged()` (selection `中`/`🙂` → Noop, no undo step/dirty);
  `fn upcase_length_changing_maps()` (`ß`→`SS`).
- `fn join_line_joins_next_with_single_space()`; `fn join_line_noop_on_last_line()`.
- `fn just_one_space_collapses_run()`; `fn just_one_space_inserts_when_none()`.
- `fn delete_blank_lines_collapses_run()`; `fn delete_horizontal_space_removes_run()`.
- `fn each_textop_is_one_undo_step()` (a representative applies as a single history entry —
  template `delete_word_back_is_one_undo_step` in `commands.rs`).

**Implementation:**
- `commands.rs`: declare the module **`pub(crate)`** (NOT private like `mod edit;`, so
  `registry.rs` — which is outside `commands` — can reach the handlers):
  ```rust
  pub(crate) mod textops;
  ```
- `commands/edit.rs`: widen `settle_after_edit` to `pub(super) fn settle_after_edit` so the
  sibling `textops` module can reuse it (it lands at `commands` scope, visible to the descendant
  `textops`). `build_range_replace`/`scope_range_at` are already `pub` in `commands.rs` and
  reachable from `textops` as `super::…`; the private `replace_changeset` is reachable from the
  child `textops` via `super::replace_changeset` (no widening needed).
- `commands/textops.rs`: each handler is
  `pub(crate) fn <id>(editor: &mut Editor, clock: &dyn Clock) -> CommandResult`, following the
  atomic-edit template. Scope for the three case ops (A11): selection-primary else word-at-caret:
  ```rust
  fn scope_or_word(editor: &Editor) -> (usize, usize) {
      let sel = editor.active().document.selection.primary();
      if !sel.is_empty() { (sel.from(), sel.to()) }
      else { super::scope_range_at(editor, crate::nav::head(editor), super::Scope::Word) }
  }
  ```
  Case op shape (upcase shown; downcase/capitalize differ only in the mapping):
  ```rust
  pub(crate) fn upcase(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
      let (from, to) = scope_or_word(editor);
      if from == to { return CommandResult::Noop; }
      let src = editor.active().document.buffer.slice(from..to); // String
      let out: String = src.chars().flat_map(char::to_uppercase).collect();
      if out == src { return CommandResult::Noop; }
      let doc_len = editor.active().document.buffer.len();
      let (cs, edit) = super::build_range_replace(from, to, &out, doc_len);
      let txn = Transaction::new(cs)
          .with_selection(Selection::range(from, from + out.len()));
      editor.apply(txn, edit, EditKind::Other, clock);
      super::edit::settle_after_edit(editor)
  }
  ```
  - `downcase`: `char::to_lowercase`. `capitalize`: per word, first char upper + rest lower
    (iterate words within `[from,to)` using `textobj::word_bounds`/`next_word_start`).
  - `transpose_chars`: caret `h = nav::head`; find the char before and after `h`
    (`buffer.slice` around `h`, multibyte-safe via `char_indices`); Noop if either missing;
    replace `[prev_start, next_end)` with `next + prev`; caret after the pair.
  - `transpose_words`: `prev_word_start`/`next_word_start` + `word_bounds` around `h`; Noop if
    two words not available; swap the two word spans, preserving the between-text; caret after
    the second word.
  - `transpose_lines`: line of `h` and the line above (via `buffer.byte_to_line`/`line_to_byte`
    + `derive::total_logical_lines`, template `delete_line`); Noop on first line; swap the two
    lines; caret at the start of the line below the swapped pair.
  - `join_line`: replace the newline + next line's leading whitespace with a single space
    (`byte_to_line`/`line_to_byte`); Noop on last line; caret at the join.
  - `just_one_space`: expand the run of `[ \t]` around `h`; replace with one space (insert one
    if none); caret after the space.
  - `delete_blank_lines`: Emacs `C-x C-o` semantics (collapse a blank run to one blank line; an
    isolated blank line → delete it; on a non-blank line → delete the following blank run);
    Noop when nothing to do.
  - `delete_horizontal_space`: delete the `[ \t]` run around `h`; Noop when none.
  All pure-delete cases may use `ChangeSet::delete(from..to, doc_len)` + `Edit { range, new_len:
  0 }` (template `delete_word`); all end with `super::edit::settle_after_edit(editor)`.
- `registry.rs::builtins`: ten direct rows (case ops → Format; other seven → `None`).
  **Placement (Codex F4): register these BEFORE the `save_settings` row** (e.g. grouped after
  the existing `delete_*` edit rows) — never appended at the end, so `save_settings` stays the
  last registered command (`journey_palette_end_reaches_last_command` + registration-order stay
  green):
  ```rust
  r.register("transpose_chars", "Transpose Characters", None, |c| crate::commands::textops::transpose_chars(c.editor, c.clock));
  r.register("transpose_words", "Transpose Words",      None, |c| crate::commands::textops::transpose_words(c.editor, c.clock));
  r.register("transpose_lines", "Transpose Lines",      None, |c| crate::commands::textops::transpose_lines(c.editor, c.clock));
  r.register("upcase",     "Uppercase",  Some(MenuCategory::Format), |c| crate::commands::textops::upcase(c.editor, c.clock));
  r.register("downcase",   "Lowercase",  Some(MenuCategory::Format), |c| crate::commands::textops::downcase(c.editor, c.clock));
  r.register("capitalize", "Capitalize", Some(MenuCategory::Format), |c| crate::commands::textops::capitalize(c.editor, c.clock));
  r.register("join_line",              "Join Line",              None, |c| crate::commands::textops::join_line(c.editor, c.clock));
  r.register("just_one_space",         "Just One Space",         None, |c| crate::commands::textops::just_one_space(c.editor, c.clock));
  r.register("delete_blank_lines",     "Delete Blank Lines",     None, |c| crate::commands::textops::delete_blank_lines(c.editor, c.clock));
  r.register("delete_horizontal_space","Delete Horizontal Space",None, |c| crate::commands::textops::delete_horizontal_space(c.editor, c.clock));
  ```
  No `Command` enum variant, no `commands::run` arm — the dispatcher and hub budgets are
  untouched (module-structure GATE). `textops` is a leaf; `#[allow(clippy::too_many_lines)]`
  only on any single fn that legitimately exceeds 100 (with a one-line reason).

**Green.** **Commit:** `feat(edit): ten atomic text-edit commands in commands/textops (A14)`.

## Task 3.4 — default keymap bindings (CUA + WordStar), conflict-checked

**Failing test:** `keymap.rs` `fn atomic_edits_and_toggle_scratch_bound_in_both_presets()`:
for each id in the eleven, assert `chord_for` resolves in a keymap built from the CUA preset AND
from the WordStar preset. `both_presets_resolve_against_builtins` (existing GATE) also covers
that every preset binding names a real command id.

**Conflict-check (verified against the real `CUA`/`WORDSTAR` tables in `keymap.rs`):**

CUA plane = `alt-` (CUA's established secondary plane). Complete used-`alt` set, re-verified
exhaustively against `keymap.rs::CUA` (all 14 alt- rows, Codex F5): `alt-left` jump_back,
`alt-right` jump_forward, `alt-o` outline, `alt-up` heading_prev, `alt-down` heading_next,
`alt-shift-up` heading_parent, `alt-z` fold_toggle, `alt-shift-z` fold_all, `alt-shift-x`
unfold_all, `alt-b` mark_block_from_selection, `alt-shift-c` copy_block_to_scratch,
**`alt-shift-v` move_block_to_scratch**, `alt-,` prev_buffer, `alt-.` next_buffer. None of the
new chords below collide with any of these (the new set is `alt-{u,l,c,t,shift-t,shift-l,j,
space,shift-j,\,s}`; in particular `alt-c` ≠ `alt-shift-c`, `alt-l`/`alt-shift-l` ≠ any used,
and none is `alt-shift-v`):

| command | CUA chord | note |
|---|---|---|
| upcase | `alt-u` | Emacs M-u |
| downcase | `alt-l` | Emacs M-l |
| capitalize | `alt-c` | Emacs M-c (≠ `alt-shift-c`=copy_block_to_scratch) |
| transpose_chars | `alt-t` | |
| transpose_words | `alt-shift-t` | |
| transpose_lines | `alt-shift-l` | (≠ `alt-l`) |
| join_line | `alt-j` | |
| just_one_space | `alt-space` | Emacs M-SPC (`space`→`Char(' ')`, verified parseable) |
| delete_blank_lines | `alt-shift-j` | |
| delete_horizontal_space | `alt-\` | Emacs M-\ (≠ `ctrl-\`=cycle_render_mode; `\`→`Char('\\')`) |
| toggle_scratch | `alt-s` | scratch (≠ `ctrl-s`=save) |

WordStar plane = the `^Q` "quick" prefix for the ten edits (precedent: `^QY`=delete_to_line_end,
a `^Q`-hosted edit) + `^K` for the buffer-family `toggle_scratch`. Both ctrl-held and plain
second-key forms, EXCEPT terminal-reserved second keys (`^J`,`^H`) → plain-only (precedent
`^KM`/`^KJ`). Verified free: used-`^Q` seconds = {s,d,r,c,e,x,f,a,l,p,y,b,k,0-9}; used-`^K`
seconds = {s,d,x,q,b,k,c,v,y,w,h,g,a,`,`,`.`,l,m,j,0-9}. My second keys are disjoint from both:

| command | WordStar chords |
|---|---|
| transpose_chars | `ctrl-q ctrl-t` / `ctrl-q t` |
| transpose_words | `ctrl-q ctrl-w` / `ctrl-q w` |
| transpose_lines | `ctrl-q ctrl-n` / `ctrl-q n` |
| upcase | `ctrl-q ctrl-u` / `ctrl-q u` |
| downcase | `ctrl-q ctrl-o` / `ctrl-q o` |
| capitalize | `ctrl-q ctrl-g` / `ctrl-q g` |
| join_line | `ctrl-q j` (plain-only; `^J` reserved) |
| just_one_space | `ctrl-q ctrl-v` / `ctrl-q v` |
| delete_blank_lines | `ctrl-q ctrl-z` / `ctrl-q z` |
| delete_horizontal_space | `ctrl-q h` (plain-only; `^H` reserved) |
| toggle_scratch | `ctrl-k ctrl-t` / `ctrl-k t` |

(Under the `^Q`/`^K` prefixes the trie resolves `^Q ^T` within the prefix subtree — it does NOT
collide with top-level `^T`=delete_word_forward, exactly as `^Q ^S` coexists with `^S`=move_left.)

`select_marked_block`: **palette-first in BOTH presets — no default chord** (decided here; the
WordStar block family MAY add `^K`-family chord later, out of scope).

**Implementation:** append the rows above to the `CUA` and `WORDSTAR` static tables in
`keymap.rs` (grouped with a `// Command-surface curation (A14/A12)` comment). No trie-engine
change — 2-chord sequences already supported.

**Green.** **Commit:** `feat(keymap): default-bind the ten atomic edits + toggle_scratch (CUA+WordStar)`.

---

# PHASE 4 — A8: dynamic Documents menu + contract amendment

## Task 4.1 — `MenuRowAction` seam (refactor; no new behavior)

Introduce the action-carrying row type WITHOUT adding any dynamic section yet — every row stays
a `Command`, so behavior is byte-identical; this isolates the type ripple for review.

**Failing test:** `menu.rs` `fn command_rows_still_dispatch_after_action_refactor()`: build the
menu; assert a known row (e.g. `reflow`) carries `MenuRowAction::Command(CommandId("reflow"))`;
and the `palette` cross-link row carries `MenuRowAction::Command(CommandId("palette"))`.

**Implementation:**
- `menu.rs`: add
  ```rust
  #[derive(Clone, Copy, PartialEq, Eq, Debug)]
  pub enum MenuRowAction { Command(crate::registry::CommandId), SwitchBuffer(crate::editor::BufferId) }
  ```
  (exhaustive — activation sites must place every variant). Change
  `MenuView.groups: Vec<(MenuCategory, Vec<(String, MenuRowAction)>)>`.
- `grouped_commands`: build static rows as `(base, value, chord, MenuRowAction::Command(id))`;
  the palette cross-link becomes `MenuRowAction::Command(CommandId("palette"))`.
- `right_justify_leaves`: make it **generic over the action carrier** so its unit tests
  (`menu_chords_are_right_justified_within_a_group`, `menu_values_share_a_right_aligned_column`,
  which pass `CommandId`) compile UNCHANGED:
  ```rust
  fn right_justify_leaves<A>(raw: Vec<(String, Option<String>, Option<String>, A)>)
      -> Vec<(String, A)> { … }   // body identical; the 4th tuple slot is opaque
  ```
- `chrome_geom.rs`: change the three menu helpers' group param type from
  `…Vec<(String, CommandId)>` to `…Vec<(String, crate::menu::MenuRowAction)>` in
  `menu_bar_layout`, `menu_dropdown_rect`, `menu_dropdown_row_at` (bodies are index-/label-only;
  only the type annotation changes).
- Activation sites dispatch on the action:
  - `menu.rs::intercept` Enter arm: `menu.groups[menu.open].1.get(menu.highlighted)` yields
    `(_, action)`; match — `Command(id)` → `dispatch_overlay_command(...)` (as today);
    `SwitchBuffer(_)` handled in Task 4.2 (add a `todo`-free arm now that is unreachable for
    Command-only rows, or `#[allow]` — implement the real arm in 4.2).
  - `mouse.rs::route_overlay` menu branch: `row_id` becomes `row_action: Option<MenuRowAction>`
    (via `menu_dropdown_row_at` → `groups.get(open)…get(row).map(|(_, a)| *a)`); dispatch on it.
- **Test/helper migration — the COMPLETE set of synthetic `(String, CommandId)` group sites**
  (Codex F3; re-grepped `grep -rn "(String,.*CommandId)" render.rs mouse.rs chrome_geom.rs
  menu.rs` — these are ALL of them, every one either builds a `MenuView.groups` or feeds a
  `chrome_geom` menu helper, so each breaks under the row-type change and must convert its leaf
  vec to `MenuRowAction::Command(CommandId(...))`):
  - `chrome_geom.rs::tall_menu_groups` (test helper) — the `leaves: Vec<(String, CommandId)>`
    it builds feeds `menu_dropdown_rect`/`menu_dropdown_row_at`; convert the leaf type to
    `MenuRowAction`.
  - `render.rs::dropdown_indicator_row_carries_panel_bg` — 20-leaf synthetic
    `Vec<(String, CommandId)>` → `MenuView.groups`; convert.
  - `render.rs::dropdown_highlight_never_hidden_in_overflow` — 20-leaf synthetic; convert.
  - `mouse.rs::menu_wheel_scrolls_dropdown` — 20-leaf synthetic → `MenuView { groups: … }`;
    convert.
  - `mouse.rs::fable_menu_setup` (test helper) — 9-leaf synthetic → `MenuView`; convert.
  - `menu.rs::group_items` (test helper) — returns `Vec<(String, CommandId)>` from
    `grouped_commands`; its return type becomes `Vec<(String, MenuRowAction)>`, and its callers
    (`build_groups_by_category_in_order_with_chords_and_palette_entry`) match on the action.
  - `menu.rs::build_groups_by_category_in_order_with_chords_and_palette_entry` and
    `menu.rs::custom_bind_surfaces_in_menu_and_palette` — change `*id == CommandId("reflow")` /
    `CommandId("cut")` reads to extract/ match `MenuRowAction::Command(CommandId(...))`.
  UNAFFECTED (verified): `menu.rs::menu_chords_are_right_justified_within_a_group` and
  `menu.rs::menu_values_share_a_right_aligned_column` pass their raw tuples to the now-GENERIC
  `right_justify_leaves<A>` with `A = CommandId`, so they compile unchanged. After editing,
  re-run the grep to confirm zero remaining non-`MenuRowAction` `(String, CommandId)` group
  constructions.

**Green.** **Commit:** `refactor(menu): rows carry MenuRowAction; generic right-justify (A8 seam)`.

## Task 4.2 — Documents dynamic section + menu-home moves

**Failing tests:**
- `workspace.rs` `fn documents_menu_rows_open_order_excludes_scratch()`: three buffers incl
  scratch; `documents_menu_rows(&e)` returns the two ordinary buffers in buffer-vec order,
  each `MenuRowAction::SwitchBuffer(id)`, labeled by `buffer_display_name`; no scratch row.
- `menu.rs` `fn documents_section_appears_and_switches()`: built menu has a `Documents` group
  whose rows are `SwitchBuffer`; activating one (drive `intercept` Enter or the mouse arm)
  makes that buffer active and closes the menu.
- `registry.rs` update `switch_buffer_is_registered_in_view_menu` → assert
  `reg.meta(CommandId("switch_buffer")).menu == None` (and it is still registered/palette-listed);
  same for `next_buffer`/`prev_buffer`.

**Implementation:**
- `registry.rs`: `MenuCategory` add `Documents` between `View` and `Settings`:
  `{ File, Edit, Block, Format, View, Documents, Settings, Export }`; `MENU_ORDER` → **8**:
  `[File, Edit, Block, Format, View, Documents, Settings, Export]`. `next_buffer`,
  `prev_buffer`, `switch_buffer` `meta.menu` → `None`. `menu.rs::category_label` add
  `Documents => "Documents"`.
- `workspace.rs`: add
  ```rust
  pub fn documents_menu_rows(editor: &Editor) -> Vec<(String, crate::menu::MenuRowAction)> {
      editor.buffers.iter()
          .filter(|b| !editor.is_scratch(b.id))
          .map(|b| (buffer_display_name(editor, b.id), crate::menu::MenuRowAction::SwitchBuffer(b.id)))
          .collect()
  }
  ```
- `menu.rs`: the registration-seam data table (NOT inline logic in `grouped_commands` —
  module-structure GATE):
  ```rust
  pub struct DynamicSection { pub category: MenuCategory,
      pub rows: fn(&crate::editor::Editor) -> Vec<(String, MenuRowAction)> }
  pub const DYNAMIC_SECTIONS: &[DynamicSection] =
      &[DynamicSection { category: MenuCategory::Documents, rows: crate::workspace::documents_menu_rows }];
  ```
  `grouped_commands`: after collecting a category's static `raw` rows, append any
  `DYNAMIC_SECTIONS` entry for that `cat` as bare `(label, None, None, action)` tuples before
  `right_justify_leaves`; keep the existing empty-group omission on the COMBINED result.
- Activation `SwitchBuffer(id)` (both `menu::intercept` Enter and `mouse.rs::route_overlay`):
  close the menu (`editor.menu = None`), then
  `if let Some(idx) = editor.buffers.iter().position(|b| b.id == id) { crate::workspace::switch_to(editor, idx); }`
  — the same shared setter registered commands use (Law 1 mutation clause satisfied; C4
  dirty-close guard untouched since no buffer is closed).

**Green.** **Commit:** `feat(menu): dynamic Documents section via DYNAMIC_SECTIONS seam (A8)`.

## Task 4.3 — apply the contract amendment (edits `command-surface-contract.md`)

This is where the governing doc is actually changed (the spec only DEFINED the text).

**No code test** (doc change). Verification = the four invariant GATEs stay green (Tasks
4.1–4.2 keep them green) + a human read.

**Implementation — edit `docs/design/command-surface-contract.md`:**
- After the "Shape rules" section, insert the **Dynamic menu sections** subsection (verbatim
  from spec §4.4): rows are data-not-commands; row ACCOUNTING is exempt from laws 1/3/4;
  ACTIONS are NOT exempt from law 1's mutation clause and must invoke a shared setter a
  registered command also uses (Documents ↔ `workspace::switch_to`); registration via the
  `DYNAMIC_SECTIONS` seam; Effort-P plugin menus = intended second consumer.
- Restate Law 4 in place: "every menu row that NAMES A COMMAND is in the palette;
  dynamic-section rows are data and exempt."
- Append to History (exact text from spec §4.4):
  > - 2026-07-10 (A8 / command-surface curation): added the **Dynamic menu sections** section —
  >   a menu category may carry rows generated from live editor state (data, not commands),
  >   exempt from laws 1/3/4 for ROW ACCOUNTING only (a row need not be a registered command or
  >   a palette entry); the row's ACTION remains bound by law 1's mutation clause and must invoke
  >   a shared setter a registered command also uses (the Documents section ↔ `workspace::switch_to`,
  >   shared with `switch_buffer`/`next_buffer`/`goto_scratch`). Restated law 4 as "every menu row
  >   that NAMES A COMMAND is in the palette." Forward-compatible with Effort-P plugin menus.

**Commit:** `docs(contract): amend for dynamic menu sections (A8 History entry)`.

---

# PHASE 5 — A13: mouse parity for minibuffer + search

## Task 5.1 — minibuffer click → caret

**Failing test:** `mouse.rs` `fn minibuffer_click_positions_caret()`: open a Filter minibuffer
with multibyte text (`prompt="sh> "`, `text="éxx"`); synth a `Down(Left)` on the status row at a
column inside the text; assert `editor.minibuffer.unwrap().cursor` is the exact byte offset of
the clicked char. `fn minibuffer_click_past_end_clamps()`; `fn minibuffer_click_prompt_is_noop()`
(caret unchanged, minibuffer stays open).

**Implementation:**
- `chrome_geom.rs`: add
  `pub(crate) fn minibuffer_click_byte(area: Rect, mb: &crate::minibuffer::Minibuffer, col: u16,
  row: u16) -> Option<usize>` — return `Some(byte)` only when `row == area.height - 1` and
  `col >= prompt_cols`; map `col - prompt_cols` char-columns into a byte offset in `mb.text`
  (char-count convention — mirror `render.rs::place_cursor`'s minibuffer arm:
  `prompt.chars().count()` then one column per char), clamped to `mb.text.len()`.
- `mouse.rs::route_overlay`: replace the tail `if editor.minibuffer.is_some() || … {}` with a
  real minibuffer branch: on `Down(Left)`, `if let Some(byte) = chrome_geom::minibuffer_click_byte(area, mb, ev.column, ev.row) { mb.cursor = byte; }`;
  all other events consumed (no-op). Keep the `search` half in Task 5.2.

**Green.** **Commit:** `feat(mouse): click-to-position-caret in the minibuffer (A13)`.

## Task 5.2 — search overlay click (field focus + match click)

**Failing tests (`mouse.rs`):**
- `fn search_needle_click_focuses_and_positions()`; `fn search_template_click_in_replace_phase()`.
- `fn search_match_click_selects_that_match_stays_open()`: buffer with two matches, click the
  second highlighted match in the body → `document.selection == Selection::range(m.start,m.end)`
  of the clicked match, `current_ordinal()` reflects it, `editor.search.is_some()` holds.
- `fn search_match_click_refreshes_stale_cache()` (spec §5.3, cache-only): with search open,
  apply an edit bumping `document.version` and shifting offsets WITHOUT re-syncing; a match-click
  recomputes against the live buffer (real current-buffer match selected, no stale offset), and
  the refresh step itself causes NO viewport/selection change (the only selection change is the
  step-3 placement). Control: a non-match body click leaves `document.selection` unchanged.

**Implementation (`mouse.rs::route_overlay` search branch + shared helpers):**
- **Field click** (status row): a `chrome_geom` helper
  `search_field_click(area, s, col, row) -> Option<(Field, usize)>` sharing the two prefix
  strings with `render.rs::place_cursor` (`"Find: "`, `"Find: {needle}  Replace: "`) — single
  source so painter/hit-test never drift. On hit: set `s.field`, `s.cursor` (char-count map,
  end-clamp). Suffix-region clicks consumed no-op.
- **Match click** (edit band), strict order (spec §5.2):
  1. **Cache-only refresh** — snapshot the live buffer + version and call `recompute` DIRECTLY
     (NOT `search_ui::search_sync`, which also unfolds/sets-selection/rebuilds/ensure_visible —
     that would move the viewport before the click is mapped):
     ```rust
     let (rope, version) = { let d = &editor.active().document; (d.buffer.snapshot(), d.version) };
     if let Some(s) = editor.search.as_mut() { s.recompute(&rope, version); }
     ```
     (`SearchState::recompute(&mut self, rope: &Rope, version: u64)` is `pub`, verified; keyed on
     `(needle, mode, case, version)` → no-op when current, rebuild when stale.)
  2. **Map** click → document byte via the existing body path
     `nav::offset_at_cell(editor, col, erow)` (same fn the no-overlay click uses; `CellHit::Text`
     from `editing_cell`).
  3. **Choose + place** — if the byte is within some `m` in `editor.search…matches()`
     (`m.start <= byte < m.end`): `s.set_current_at_or_after(m.start)` then the shared placement
     tail (extract the post-`recompute` body of `search_sync`/`search_step` into a
     `search_ui` helper — `unfold_ancestors_of(editor, m.start)`; selection =
     `Selection::range(m.start, m.end)`; `derive::rebuild`; `nav::ensure_visible`). Overlay STAYS
     OPEN. Non-match/other events consumed no-op.

**Green.** **Commit:** `feat(mouse): click-to-focus fields + click-to-select match in search (A13)`.

---

# PHASE 6 — merge verification

## Task 6.1 — 8-category menu-bar small-terminal verify (before-merge)

The bar grows to 8 categories ≈ 62 columns (`File 6 + Edit 6 + Block 7 + Format 8 + View 6 +
Documents 11 + Settings 10 + Export 8`); `chrome_geom::menu_bar_layout_cats` has NO horizontal
windowing (verified — only the dropdown has vertical windowing). Below ~62 cols the trailing
categories clip and become mouse-unreachable (keyboard Left/Right still cycles via
`menu::intercept`).

**Verification (not a code change unless a defect is found):**
- Drive the live bar at the smallest supported width. Use the PTY smoke harness
  (`scripts/smoke/run.sh`) and/or an in-process `e2e.rs` `TestBackend` journey at a narrow width
  (e.g. 60×24): open the menu, Left/Right cycle to `Export`, confirm keyboard reach; observe the
  clip.
- Quote `scripts/smoke/run.sh`'s one-line summary in the pre-merge report (mandatory-run,
  advisory-pass).
- If the clip is judged unacceptable at the min size, file bar-overflow handling as a NEW
  backlog item (`bl:`) — do NOT expand this effort's scope silently.

**No commit unless a fix lands.**

---

## Pre-merge report checklist (final gates — both must pass)

- `cargo test` green across `wordcartel-core` (lib + oracle) and `wordcartel` (lib + e2e +
  `tests/module_budgets.rs` + `tests/backlog.rs`).
- `cargo clippy --workspace --all-targets` clean (deny).
- `cargo build` + `cargo test --no-run` warning-free for touched crates.
- Invariant GATEs: `palette_is_exhaustive_over_the_registry`,
  `every_persisted_setting_has_a_command`, `hints_reresolve_on_preset_switch`,
  `custom_bind_surfaces_in_menu_and_palette`, `both_presets_resolve_against_builtins`.
- `scripts/smoke/run.sh` one-line summary quoted verbatim (advisory).
- Fable whole-branch review + Codex pre-merge GO.
- Backlog: mark A8/A9/A10/A11/A12/A13/A14/A3b per state in `backlog.toml` → `scripts/backlog
  bless`; move shipped prose to `docs/backlog-archive.md`.

---

## Flags — human decisions carried from the spec (surface, do NOT silently resolve)

These were flagged OPEN in the spec (§ "Flagged items") and are NOT resolved by Codex's GO on
the spec text. The plan implements the spec's recommended end state but marks each for a human
ruling at review:

1. **A9 `MenuMark::Text(String)` + drop `Copy`** (Task 3.1) — the ledger said `MenuMark::Value`;
   `Value` is `&'static str` and cannot carry a runtime `u16`. The plan adds `Text(String)`.
   (Ripple is contained: one match site; `CommandMeta: Copy` unaffected.)
2. **A3b FLAG 1 — `transform` View → Format** (Task 3.2). Recommended; reversible one-line
   `meta.menu` edit.
3. **A3b FLAG 2 — `delete_word_back`/`delete_word_forward`/`delete_line`/`delete_to_line_end`
   Edit → palette-only** (Task 3.2). Recommended for A14 coherence; reversible.
4. **Filter shell = POSIX `sh -c`** (Task 1.2) — not `$SHELL -c`; the existing
   `run_subprocess` mechanism hardcodes `sh`. Deterministic across user shells.
5. **`goto_scratch` also records the return buffer** (Task 2.2) — coherence extension so
   goto→toggle round-trips.

Additional plan-level decisions (not spec flags, decided here per the binding-policy mandate):
- **CUA plane = `alt-`** for the ten edits + `toggle_scratch` (CUA already uses `alt-`
  extensively). `just_one_space`→`alt-space` and `delete_horizontal_space`→`alt-\` rely on the
  terminal delivering those chords; if a terminal cannot, the commands stay palette-reachable
  (Law 3). Minor caveat, not blocking.
- **WordStar plane = `^Q` prefix** for the ten edits (precedent `^QY`) + `^K t` for
  `toggle_scratch` (buffer family). `^K` was too crowded to host eleven cleanly (only ~10 free
  second keys); `^Q` is the correct "quick-edit" home and is conflict-free (check shown in Task
  3.4). This is a slight departure from the spec's shorthand "WordStar is `^K`-prefix" — FLAGGED
  for confirmation.
- **`select_marked_block` palette-only in both presets** (no default chord) — honoring
  "palette-first"; WordStar block-family chord deferred.
