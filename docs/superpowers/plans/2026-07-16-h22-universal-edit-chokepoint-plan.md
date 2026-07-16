# H22 — Universal Edit Chokepoint (implementation plan)

**For agentic workers:** Execute tasks top-to-bottom. Each task is TDD: write the failing test →
run it (confirm it fails for the stated reason) → write the minimal implementation → run (confirm
green) → run the surface's regression subset → commit. Do NOT reorder: the `Buffer::apply` demotion
+ INV-SEAM scan test (Task 7) only pass AFTER every surface is migrated. Anchor edits by SYMBOL
NAME, not the line numbers here (they drift as tasks edit files) — re-locate with `grep`/LSP. For any
compile/usage/signature question on code you are editing, trust `cargo` + `grep`, not an editor
"unused"/"undefined" hint. This plan is grounded in the GO-gated spec
`docs/superpowers/specs/2026-07-16-h22-universal-edit-chokepoint-design.md` (commit 94c001d).

## Goal
Route **all** internal buffer edits through one funnel (`edit_apply::apply_edit`) so versioning, the
read-only guard, the reparse/viewport epilogue, and the Effort-P plugin-edit seam live at a single,
compiler-guarded seam instead of being re-implemented per call site.

## Architecture
`edit_apply::apply_edit(editor, buffer_id, txn, edit, kind, clock) -> EditOutcome` is the shared
inner core: it runs the loud read-only guard, a debug validation backstop, the single
`Buffer::apply` mutation, and the active-buffer epilogue (`resettle` = rebuild + ensure_visible +
`desired_col=None`). `submit_transaction` is the validated shell on top ("validate → core");
internal callers call the core pre-trusted. `Buffer::apply` is demoted to `pub(crate)` so the
compiler blocks every out-of-crate bypass (the Effort-P concern), with a source-scan test as the
in-crate regression tripwire.

## Tech Stack
Rust workspace: pure `wordcartel-core` (`#![forbid(unsafe_code)]`, no IO) + `wordcartel` shell
(binary `wcartel`; ratatui 0.30, crossterm). `ropey` ropes; `mlua` plugins (untouched here).

## Global Constraints (binding — copied verbatim from CLAUDE.md + the spec)
- **House style, hand-formatted.** Do NOT run `cargo fmt` (no `rustfmt.toml`; it would reflow the
  whole tree). Match neighbors by hand: 4-space indent, ~100-char hand-wrapped lines, `—` em-dash in
  prose comments (never `--`), snake_case fns, no emoji in code (multibyte only in tests exercising
  `é`/`中`/`🙂`). Private fields by default; `Option<T>` over sentinels; typed error enums to the
  status line, never the console.
- **Merge GATEs:** `cargo test` green across all suites; `cargo build` + `cargo test --no-run`
  warning-free for touched crates; **workspace clippy clean** (`cargo clippy --workspace
  --all-targets`, `[workspace.lints.clippy] all = "deny"`); **`module_budgets`**
  (`wordcartel/tests/module_budgets.rs`); **backlog-drift** (`wordcartel/tests/backlog.rs`). A
  deliberate clippy exception is an item-local `#[allow(clippy::…)]` with a one-line rationale, never
  a blanket allow.
- **`too_many_lines` = 100** (`clippy.toml`): any fn over 100 lines splits, or carries an item-local
  `#[allow(clippy::too_many_lines)]` with a one-line reason. `edit_apply.rs` is a thin seam — no
  dispatch attractor; keep `apply_edit`/`resettle` short.
- **Commit trailers** — every commit ends with, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG
  ```
- **Command-surface-contract: N/A-with-caveat.** H22 adds/removes/renames no commands, options,
  palette rows, menu entries, or hints; the registry/palette/menu/hint code is untouched. The F4
  loud-read-only change touches status FEEDBACK on paths unreachable-today on a read-only buffer
  (`read_only` set only at the `view_messages` history buffer, `editor.rs:193`), not the
  command/palette/menu/hint surface; the contract's invariant tests are unaffected. State this in the
  final commit.
- **The four funnel invariants (spec §4):**
  - **INV-SEAM** — `Buffer::apply` is `pub(crate)`, sole in-crate caller `edit_apply::apply_edit`.
  - **INV-GUARD** — every internal edit to a read-only buffer returns `RejectedReadOnly` with the
    canonical Sticky Warning set once; no caller acks success on that outcome.
  - **INV-EPILOGUE** — after an active-buffer edit the tree is reparsed and the caret made visible
    before control returns; `desired_col=None` is owned by `resettle` (no direct writes elsewhere on
    edit paths).
  - **INV-LAZY-HEAL** — a non-active edited buffer's tree MAY lag; every activation path (`advance()`
    pre-draw rebuild `app.rs:398`, `workspace::switch_to` `workspace.rs:52`) heals it before its
    first render (proportional-to-work; do NOT eager-reparse).
- **Ordering constraint (Surface C):** `corrected_after_move` (`blocks_marked.rs:55` /
  `prose_ops.rs:150`) MUST stay computed from the PRE-edit folds + `cs`, i.e. **before**
  `Transaction::new(cs)` and the `apply_edit`/core call. Do NOT reorder it after apply during
  migration — the corrected set is defined against pre-edit fold byte-offsets.

---

## Task 1 — Create `edit_apply.rs`: `EditOutcome` + `apply_edit` core + `resettle`; widen `Editor::apply`

**Deliverable:** the funnel module exists and `Editor::apply` delegates to it (returning
`EditOutcome`), with the epilogue relocated to `edit_apply::resettle`. All existing tests stay green
(migrated surfaces still carry their own now-redundant epilogues until later tasks — the transient
double run converges to an identical final state, m-P1).

**Interfaces**
- *Produces:* `pub mod edit_apply;` (in `wordcartel/src/lib.rs`); `pub enum EditOutcome { Applied,
  RejectedReadOnly, BufferGone }`; `pub fn apply_edit(editor: &mut Editor, buffer_id: BufferId, txn:
  Transaction, edit: Edit, kind: EditKind, clock: &dyn Clock) -> EditOutcome`; `pub(crate) fn
  resettle(editor: &mut Editor)`.
- *Consumes (real signatures):* `Editor::by_id(&self, id: BufferId) -> Option<&Buffer>`
  (`editor.rs:711`); `Editor::by_id_mut(&mut self, id) -> Option<&mut Buffer>` (`editor.rs:712`);
  `Editor::active(&self) -> &Buffer` (`editor.rs:684`); `Editor::active_mut(&mut self) -> &mut Buffer`
  (`editor.rs:688`); `Editor::reject_read_only(&mut self)` (`editor.rs:1078`); `Buffer::apply(&mut
  self, txn: Transaction, edit: Edit, kind: EditKind, clock: &dyn Clock)` (`editor.rs:267`, still
  `pub` until Task 7); `Buffer::read_only: bool` (`editor.rs:193`); `Buffer::document.buffer:
  TextBuffer`; `Transaction.changes: ChangeSet` (`history.rs:54`, pub); `ChangeSet::validate_against(&self,
  buf: &TextBuffer) -> Result<(), EditError>` (`change.rs:164`); `derive::rebuild(&mut Editor)`
  (`derive.rs:91`); `nav::ensure_visible(&mut Editor)` (`nav.rs:388`); `Buffer::desired_col:
  Option<usize>` (`editor.rs:145`). Existing epilogue to relocate: `commands::edit::settle_after_edit`
  (`commands/edit.rs:21–26`: `derive::rebuild` + `nav::ensure_visible` + `active_mut().desired_col =
  None`, returns `CommandResult::Handled`).

**TDD steps**
1. **Failing test (compiling stub → red on the assertion, per TDD hygiene I-P2).** Create
   `wordcartel/src/edit_apply.rs` with (a) a MINIMAL COMPILING STUB and (b) the `#[cfg(test)] mod
   tests`, and declare the module so the file is compiled:
   - Add `pub mod edit_apply;` to `wordcartel/src/lib.rs` (alongside `pub mod editor;`).
   - Write the stub — enough to compile so the test fails on the ASSERTION, not on a missing symbol:
     ```rust
     use crate::editor::{BufferId, Editor};
     use wordcartel_core::block_tree::Edit;
     use wordcartel_core::history::{Clock, EditKind, Transaction};

     #[derive(Clone, Copy, PartialEq, Eq, Debug)]
     pub enum EditOutcome { Applied, RejectedReadOnly, BufferGone }

     // STUB (Task 1 step 2 replaces this body). Compiles; returns the wrong outcome so the
     // `active_edit_applies_and_runs_epilogue` assertion is genuinely RED.
     pub fn apply_edit(editor: &mut Editor, buffer_id: BufferId, txn: Transaction, edit: Edit,
                       kind: EditKind, clock: &dyn Clock) -> EditOutcome {
         let _ = (editor, buffer_id, txn, edit, kind, clock);
         EditOutcome::BufferGone
     }
     ```
   - Add the test module (note: `Buffer` is NOT imported here — no Task-1 test uses it; the
     lazy-heal test that needs it lands in Task 7 and imports it locally, m-P2):
   ```rust
   #[cfg(test)]
   mod tests {
       use super::*;
       use crate::editor::Editor;
       use wordcartel_core::history::{Clock, EditKind, Transaction};

       struct C(u64);
       impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }

       fn ins(doc_len: usize) -> (wordcartel_core::change::ChangeSet, Edit) {
           crate::commands::build_multi_replace(&[(0, 0, "X".into())], doc_len)
       }

       #[test]
       fn active_edit_applies_and_runs_epilogue() {
           let mut e = Editor::new_from_text("abc\n", None, (80, 24));
           let id = e.active().id;
           let (cs, edit) = ins(e.active().document.buffer.len());
           let out = apply_edit(&mut e, id, Transaction::new(cs), edit, EditKind::Other, &C(0));
           assert_eq!(out, EditOutcome::Applied);
           assert_eq!(e.active().document.buffer.to_string(), "Xabc\n");
           // Epilogue ran on the active buffer: tree reparsed (blocks_version caught up).
           assert_eq!(e.active().reconcile.blocks_version, e.active().document.version);
       }

       #[test]
       fn read_only_target_is_a_loud_reject_no_mutation() {
           let mut e = Editor::new_from_text("keep\n", None, (80, 24));
           let id = e.active().id;
           e.active_mut().read_only = true;
           let before = e.active().document.buffer.to_string();
           let (cs, edit) = ins(e.active().document.buffer.len());
           let out = apply_edit(&mut e, id, Transaction::new(cs), edit, EditKind::Other, &C(0));
           assert_eq!(out, EditOutcome::RejectedReadOnly);
           assert_eq!(e.active().document.buffer.to_string(), before);
           assert_eq!(e.status_text(), "buffer is read-only");
       }

       #[test]
       fn missing_buffer_is_buffer_gone_no_status() {
           let mut e = Editor::new_from_text("x\n", None, (80, 24));
           let (cs, edit) = ins(1);
           let out = apply_edit(&mut e, crate::editor::BufferId(9999),
               Transaction::new(cs), edit, EditKind::Other, &C(0));
           assert_eq!(out, EditOutcome::BufferGone);
           assert_eq!(e.status_text(), "", "BufferGone sets no status");
       }
   }
   ```
   Run `cargo test -p wordcartel edit_apply` → the module COMPILES (stub + declaration) and
   `active_edit_applies_and_runs_epilogue` **fails on its assertion** (`BufferGone != Applied`) — a
   genuine red. (`missing_buffer_is_buffer_gone_no_status` happens to pass against the stub; that is
   fine — step 2 makes all three pass for the right reasons.)
2. **Implement** — replace the step-1 stub IN FULL with the complete module below (adds the module
   doc-comment + `resettle` + the real `apply_edit` body; the `use`s and `EditOutcome` enum are the
   same as the stub's). `pub mod edit_apply;` is already in lib.rs from step 1.
   ```rust
   //! The one funnel every internal buffer edit passes through (H22, decision B). Owns the loud
   //! read-only guard, a debug validation backstop, the single `Buffer::apply` mutation, and the
   //! active-buffer epilogue (`resettle`). `submit_transaction` validates then calls here; internal
   //! callers call here pre-trusted. `Buffer::apply` is `pub(crate)` and MUST be called ONLY from
   //! this module — the compiler-guarded no-bypass seam (INV-SEAM, enforced by `tests/edit_seam.rs`).

   use crate::editor::{BufferId, Editor};
   use wordcartel_core::block_tree::Edit;
   use wordcartel_core::history::{Clock, EditKind, Transaction};

   /// Outcome of a funnelled edit — callers gate their status acks on this (INV-GUARD, F4).
   #[derive(Clone, Copy, PartialEq, Eq, Debug)]
   pub enum EditOutcome {
       /// The edit committed.
       Applied,
       /// The target buffer is read-only; the canonical Sticky Warning was set here, nothing mutated.
       RejectedReadOnly,
       /// The target buffer id was not found (raced close/dispose); nothing mutated, no status.
       BufferGone,
   }

   /// The shared post-edit epilogue (F2=A — relocated from `commands::edit::settle_after_edit`):
   /// re-derive the block tree, re-scroll to the caret, reset vertical-motion memory. Operates on
   /// the ACTIVE buffer. The core runs it after an active edit; the two fold-correction commands
   /// (`block_move`/`swap`, §3.6) call it as their post-`replace_folded` rebuild #2.
   pub(crate) fn resettle(editor: &mut Editor) {
       crate::derive::rebuild(editor);
       crate::nav::ensure_visible(editor);
       editor.active_mut().desired_col = None;
   }

   /// THE internal-edit funnel. Applies `txn`/`edit` to `buffer_id` (not necessarily active) through
   /// the single mutation channel, then runs the epilogue iff the edited buffer is active. Pre-trusted:
   /// the changeset is assumed valid-by-construction (built from a live `doc_len`); a `debug_assert`
   /// backstops that (F5). Never panics in release, never partially edits.
   ///
   /// # Examples
   /// ```
   /// # use wordcartel::editor::Editor;
   /// # use wordcartel::edit_apply::{apply_edit, EditOutcome};
   /// # use wordcartel_core::history::{Clock, EditKind, Transaction};
   /// # struct C; impl Clock for C { fn now_ms(&self) -> u64 { 0 } }
   /// let mut e = Editor::new_from_text("hi\n", None, (40, 6));
   /// let id = e.active().id;
   /// let (cs, edit) = wordcartel::commands::build_multi_replace(&[(0, 0, "X".into())], 3);
   /// assert_eq!(apply_edit(&mut e, id, Transaction::new(cs), edit, EditKind::Other, &C),
   ///            EditOutcome::Applied);
   /// assert_eq!(e.active().document.buffer.to_string(), "Xhi\n");
   /// ```
   pub fn apply_edit(
       editor: &mut Editor,
       buffer_id: BufferId,
       txn: Transaction,
       edit: Edit,
       kind: EditKind,
       clock: &dyn Clock,
   ) -> EditOutcome {
       // INV-GUARD (F4): uniform loud read-only. Absent buffer → BufferGone (no status).
       match editor.by_id(buffer_id) {
           None => return EditOutcome::BufferGone,
           Some(b) if b.read_only => { editor.reject_read_only(); return EditOutcome::RejectedReadOnly; }
           Some(_) => {}
       }
       // F5 (H7 blast-radius stance): debug-only backstop that the pre-trusted changeset applies
       // cleanly against the live target text. Release = trust-by-construction, zero cost.
       #[cfg(debug_assertions)]
       {
           let b = editor.by_id(buffer_id).expect("buffer presence checked above");
           debug_assert!(
               txn.changes.validate_against(&b.document.buffer).is_ok(),
               "internal edit built an invalid changeset for {buffer_id:?}",
           );
       }
       // Mutate through the single channel (scoped borrow — the transform.rs:290–300 borrow-split
       // made canonical). The borrow ends before the epilogue's `&mut Editor` calls below.
       {
           let b = editor.by_id_mut(buffer_id).expect("buffer presence checked above");
           b.apply(txn, edit, kind, clock);
       }
       // INV-EPILOGUE (F2) / INV-LAZY-HEAL (F3): epilogue on the ACTIVE buffer only. A non-active
       // edit leaves a lagging tree that every activation path heals before its first render.
       if buffer_id == editor.active().id {
           resettle(editor);
       }
       EditOutcome::Applied
   }
   ```
3. **Relocate the epilogue.** Rewrite `commands::edit::settle_after_edit` (`commands/edit.rs:21–26`)
   to delegate to the new shared helper, preserving its `CommandResult` return and `pub(super)`
   visibility (its callers are unchanged this task):
   ```rust
   /// Post-edit epilogue for the buffer-edit primitives — now a thin delegate to the shared core
   /// epilogue `edit_apply::resettle` (H22 F2=A). Retained so `swap`'s rebuild #2 (prose_ops.rs)
   /// keeps a `CommandResult`-returning re-settle; standard primitives stop calling it once the
   /// core owns the epilogue (Task 4).
   pub(super) fn settle_after_edit(editor: &mut Editor) -> CommandResult {
       crate::edit_apply::resettle(editor);
       CommandResult::Handled
   }
   ```
   (Remove `settle_after_edit`'s now-duplicate `use crate::derive; use crate::nav;` reliance only if
   they become unused in `edit.rs` — verify with `cargo build`, do NOT assume.)
4. **Widen `Editor::apply` (J2).** Change the delegator (`editor.rs:1062–1065`) from returning `()`
   to delegating to the core:
   ```rust
   pub fn apply(&mut self, txn: Transaction, edit: wordcartel_core::block_tree::Edit, kind: EditKind,
                clock: &dyn Clock) -> crate::edit_apply::EditOutcome {
       let id = self.active().id;
       crate::edit_apply::apply_edit(self, id, txn, edit, kind, clock)
   }
   ```
   This routes every existing `editor.apply(...)` client through the core immediately (they keep
   their own epilogues until Tasks 3–5 — the double run CONVERGES to an identical final state:
   `derive::rebuild` is LayoutKey-gated (`derive.rs:246`), but the second `nav::ensure_visible` can
   re-scroll (`nav.rs:461`) and is idempotent, so the pair settles the same viewport). All are
   statement-position callers, so the `()`→`EditOutcome` widening compiles unchanged.
5. **Run** `cargo test -p wordcartel edit_apply` → green; then `cargo test -p wordcartel`
   (full shell suite) → green (the transient double epilogue converges to an identical final state —
   see step 4). `cargo clippy
   --workspace --all-targets` → clean.
6. **Commit:** `H22 Task 1: edit_apply core (apply_edit/EditOutcome/resettle) + Editor::apply delegates`.

---

## Task 2 — Surface S: `submit_transaction` = validate → core (F4 return mapping)

**Deliverable:** the untrusted shell delegates its terminal apply to the core and maps `EditOutcome`
to its `Result<(), EditError>`; the plugin API sites are untouched.

**Interfaces**
- *Consumes:* `submit_transaction(editor: &mut Editor, txn: Transaction, clock: &dyn Clock) ->
  Result<(), EditError>` (`transact.rs:12`); its terminal `editor.apply(final_txn, edit,
  EditKind::Other, clock)` (`transact.rs:43`); `edit_apply::apply_edit` (Task 1).
- *Produces:* unchanged public signature of `submit_transaction`. `wc.replace`/`wc.insert`
  (`plugin/api.rs:349/377/404`) call `submit_transaction` and are UNTOUCHED.

**TDD steps**
1. **Regression guard (NOT red-first — I-P1).** Task 2 is a behavior-preserving refactor: after Task 1,
   `submit_transaction` already routes through the core (via `Editor::apply` → `apply_edit`), and the
   ORIGINAL `Editor::apply` was ALREADY loud on read-only (`reject_read_only()` before delegating,
   `editor.rs:1063`). So the loud-reject + `Ok(())` contract is GREEN both before and after this task —
   the change is purely the mechanical switch to calling `apply_edit` directly and mapping its
   `EditOutcome` to `Result`. Add these as characterization guards (confirm green BEFORE the edit, and
   still green after — they lock the contract the refactor must preserve). The existing `transact.rs`
   proptest + units (`valid_transaction_applies`, `stale_length_rejected_no_mutation`,
   `out_of_bounds_selection_snaps_not_rejects`, `transact.rs:57–97`) are the primary guard; add:
   ```rust
   // Regression guard (green before AND after Task 2): the validate→core refactor preserves the
   // loud read-only refusal and the Ok(()) return (a read-only view's edit is cleanly declined).
   #[test]
   fn submit_into_read_only_is_ok_after_loud_reject() {
       let mut e = ed("hello\n");
       e.active_mut().read_only = true;
       let before = e.active().document.buffer.to_string();
       let cs = ChangeSet::insert(0, "X", 6);
       let r = submit_transaction(&mut e, Transaction::new(cs), &C(0));
       assert!(r.is_ok(), "read-only edit is refused loudly, not errored");
       assert_eq!(e.active().document.buffer.to_string(), before, "no mutation");
       assert_eq!(e.status_text(), "buffer is read-only");
   }
   ```
   Run BEFORE the edit → **passes** (the original delegator is already loud). This is the honest
   framing: Task 2 has no genuinely-red behavioral delta; the guard locks the preserved contract, and
   the existing proptest guards the validate→core mapping (valid applies, stale/boundary reject
   without mutation).
2. **Implement** — replace `transact.rs:41–44` (the `final_txn` build + `editor.apply` + `Ok(())`):
   ```rust
       // 4. Build the final transaction (original changes + snapped selection) and apply once via
       //    the core — the only live mutation. A read-only active buffer is refused loudly here
       //    (INV-GUARD); the untrusted boundary still returns Ok — the edit was cleanly declined.
       let mut final_txn = Transaction::new(changes);
       if let Some(sel) = snapped_sel { final_txn = final_txn.with_selection(sel); }
       let id = editor.active().id;
       let _ = crate::edit_apply::apply_edit(editor, id, final_txn, edit, EditKind::Other, clock);
       Ok(())
   ```
   (The `EditOutcome` is intentionally discarded: `Applied`/`RejectedReadOnly`/`BufferGone` all map to
   `Ok(())` — validation already rejected malformed changesets before this point; a read-only refusal
   is a clean decline, and the active buffer always exists so `BufferGone` is unreachable.)
3. **Run** `cargo test -p wordcartel transact` → green (incl. the 2048-case proptest). Full
   `cargo test -p wordcartel` → green. Clippy clean.
4. **Commit:** `H22 Task 2: submit_transaction validates then calls the core (F4 loud read-only)`.

---

## Task 3 — Surface A: migrate the 7 raw `Buffer::apply` bypass sites

**Deliverable:** transform-merge, filter-done, paste, the three search actions, and scratch-append
route through the core; their hand-rolled epilogues are removed; the `apply_filter_done` false-ack is
fixed (gated on `Applied`).

**Interfaces**
- *Consumes:* `edit_apply::apply_edit` + `EditOutcome`; `Editor::apply` (for active-buffer sites).
  Sites: `merge_transform_into` (`transform.rs:262`, edits `by_id`); `apply_filter_done`
  (`jobs_apply.rs:211`); `insert_paste_text` (`jobs_apply.rs:336`); `search_replace_all`
  (`search_ui.rs:29`), `search_step_apply` (`:65`), `search_step_rest` (`:98`); `append_to_scratch`
  (`scratch.rs:11`).
- *Produces:* no signature changes; `insert_paste_text` still returns `bool`; `append_to_scratch`
  still returns `bool`.

**TDD steps (one edit + green run per site; commit once at the end)**
1. **Failing test (the headline fix)** — add to `jobs_apply.rs` tests the INV-GUARD false-ack pin:
   ```rust
   #[test]
   fn apply_filter_done_into_read_only_does_not_false_ack() {
       use crate::editor::Editor;
       let mut e = Editor::new_from_text("keep\n", None, (80, 24));
       let id = e.active().id;
       let v = e.active().document.version;
       e.active_mut().read_only = true;
       let before = e.active().document.buffer.to_string();
       apply_filter_done(&mut e, id, v, 0..1, 0, crate::filter::Disposition::Filter,
           crate::filter::RunResult::Stdout("X".into()), &TestClock(0));
       assert_ne!(e.status_text(), "filter applied", "no success ack on a read-only reject");
       assert_eq!(e.status_text(), "buffer is read-only");
       assert_eq!(e.active().document.buffer.to_string(), before, "no mutation");
   }
   ```
   Run → **fails**: today `apply_filter_done` calls `b.apply` (silent no-op on read-only) then still
   emits "filter applied".
2. **Implement, site by site:**
   - **`merge_transform_into` (`transform.rs:277–303`, the `Ok(out)` branch):** replace the
     `by_id_mut`→`b.apply` + active-gated `derive::rebuild`/`ensure_visible` (`:291–300`) with the
     core; keep the no-op check (`:282`) and the `finish_topic` acks:
     ```rust
             let (cs, edit) = crate::commands::build_range_replace(range.start, range.end, &out, doc_len);
             let txn = wordcartel_core::history::Transaction::new(cs);
             match crate::edit_apply::apply_edit(editor, buffer_id, txn, edit,
                                                 wordcartel_core::history::EditKind::Other, clock) {
                 crate::edit_apply::EditOutcome::Applied => {
                     editor.finish_topic(crate::status::StatusTopic::Transform,
                         crate::status::StatusKind::Info, kind.past_tense().to_string());
                 }
                 // Buffer vanished mid-flight, or (unreachable — dispatch guards read-only) refused.
                 crate::edit_apply::EditOutcome::BufferGone
                 | crate::edit_apply::EditOutcome::RejectedReadOnly => {}
             }
     ```
     (The core runs the epilogue only when `buffer_id` is active — identical to the old active-gate.)
   - **`apply_filter_done` (`jobs_apply.rs:235–256`, the `Stdout` arm):** replace the
     `by_id_mut`→`apply`→`bool` + `if apply_result { rebuild; ensure_visible; finish_topic }` with:
     ```rust
           crate::filter::RunResult::Stdout(text) => {
               let (from, to, at) = match disposition {
                   crate::filter::Disposition::Filter => (range.start, range.end, range.start),
                   crate::filter::Disposition::Insert => (cursor, cursor, cursor),
               };
               let doc_len = editor.by_id(buffer_id).map(|b| b.document.buffer.len());
               if let Some(doc_len) = doc_len {
                   let (cs, edit) = crate::commands::build_range_replace(from, to, &text, doc_len);
                   let txn = wordcartel_core::history::Transaction::new(cs)
                       .with_selection(wordcartel_core::selection::Selection::single(at + text.len()));
                   if crate::edit_apply::apply_edit(editor, buffer_id, txn, edit,
                          wordcartel_core::history::EditKind::Other, clock)
                       == crate::edit_apply::EditOutcome::Applied
                   {
                       editor.finish_topic(crate::status::StatusTopic::Filter,
                           crate::status::StatusKind::Info, "filter applied");
                   }
               }
           }
     ```
   - **`insert_paste_text` (`jobs_apply.rs:352–368`):** keep the paste-size + entry read-only guards
     (`:342–351`). Replace the scoped `by_id_mut` mutation + `b.desired_col = None` + active-gated
     epilogue with the core; the `bool` becomes `Applied`:
     ```rust
       let sel_from = editor.by_id(buffer_id).map(|b| {
           let sel = b.document.selection.primary(); (sel.from(), sel.to())
       });
       let Some((from, to)) = sel_from else { return false; };
       let doc_len = editor.by_id(buffer_id).map(|b| b.document.buffer.len()).unwrap_or(0);
       let (cs, edit) = crate::commands::build_range_replace(from, to, text, doc_len);
       let txn = wordcartel_core::history::Transaction::new(cs)
           .with_selection(wordcartel_core::selection::Selection::single(from + text.len()));
       matches!(
           crate::edit_apply::apply_edit(editor, buffer_id, txn, edit,
               wordcartel_core::history::EditKind::Other, clock),
           crate::edit_apply::EditOutcome::Applied
       )
     ```
     (`desired_col=None` now happens inside `resettle` when the paste targets the active buffer; a
     non-active paste target follows INV-LAZY-HEAL. The core's read-only guard is redundant with the
     entry guard at `:342` — harmless defense-in-depth.)
   - **`search_replace_all` (`search_ui.rs:57–62`):** keep the entry read-only guard (`:30`). Replace
     `editor.active_mut().apply(...)` + trailing `derive::rebuild`/`ensure_visible` (`:61–62`) with the
     Editor delegator (active buffer):
     ```rust
       editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
       if let Some(s) = editor.search.as_mut() { s.origin = new_origin; }
       editor.set_status(crate::status::StatusKind::Info, format!("Replaced {n} occurrences"));
       editor.search = None; // close after replace-all
     ```
     (`editor.apply` routes through the core, which runs `resettle` — the old `:61–62` epilogue.)
   - **`search_step_apply` (`search_ui.rs:79`):** replace `editor.active_mut().apply(...)` with
     `editor.apply(...)`. The re-find block (`:80–88`) and `search_pin` (`:88`) stay; `search_pin`'s
     own `rebuild` (`search_ui.rs:129`) is now redundant with the core epilogue (it re-derives the
     already-reparsed post-edit tree; LayoutKey-gated, converging to the same state).
   - **`search_step_rest` (`search_ui.rs:114–116`):** replace `editor.active_mut().apply(...)` +
     trailing `derive::rebuild(editor); crate::nav::ensure_visible(editor);` (`:116`) with
     `editor.apply(...)`.
   - **`append_to_scratch` (`scratch.rs:21`):** replace `editor.by_id_mut(sid).unwrap().apply(...)`
     with the core (non-active scratch → INV-LAZY-HEAL, no epilogue):
     ```rust
       matches!(
           crate::edit_apply::apply_edit(editor, sid, txn, edit,
               wordcartel_core::history::EditKind::Other, clock),
           crate::edit_apply::EditOutcome::Applied
       )
     ```
     (`append_to_scratch` already early-returns `false` when no scratch is installed at `:12–13`; a
     surviving scratch yields `Applied`.)
3. **Run** the failing test → green; then `cargo test -p wordcartel transform search jobs_apply
   scratch` + full `cargo test -p wordcartel` → green. Clippy clean.
4. **Commit:** `H22 Task 3: migrate the 7 raw Buffer::apply bypass sites onto the core (fixes filter false-ack)`.

---

## Task 4 — Surface B: drop the redundant epilogues from standard `Editor::apply` clients

**Deliverable:** every standard active-buffer edit client stops hand-rolling the epilogue (the core
owns it); `diag_apply_selected`'s double-rebuild collapses; `blocks_marked::apply_edit` helper loses
its manual epilogue.

**Interfaces**
- *Consumes:* `Editor::apply` (core-backed since Task 1); `edit_apply::resettle`. Sites:
  `commands/edit.rs` primitives (`:41…:246` + their `settle_after_edit` tails); `commands/textops.rs`
  (`:47…:435`); `prose_ops::{move_sentence:91, break_paragraph_here:197, merge_paragraph_forward:250,
  split_sentence_at_caret:293}` (+ their `settle_after_edit` at `:92/:198/:251/:294`);
  `blocks_marked::apply_edit` helper (`:80–91`, callers `block_copy:30`, `block_delete:74`,
  `block_move:57`); `move_block_to_scratch` (`scratch.rs:57`, epilogue `:59–60`); `diag_apply_selected`
  (`search_ui.rs:135`, apply `:208`, double-rebuild `:209/:211`).
- *Produces:* no signature changes; `CommandResult` returns preserved.

**TDD steps**
1. **REGRESSION GUARD (INV-EPILOGUE — green BEFORE and AFTER; not red-first).** This task is a
   behavior-preserving cleanup (the removed settle was a redundant epilogue), so its guards
   characterize preserved behavior rather than fail first. The existing
   `derive::keystroke_runs_layout_once` (`derive.rs:803`) already asserts exactly one layout run
   across the post-command + pre-draw rebuild for a mid-screen insert; it must stay green after the
   epilogue relocation. Additionally add a targeted assertion that a migrated primitive re-derives
   correctly (proves the core epilogue fires without the manual one):
   ```rust
   // in commands/edit.rs tests
   #[test]
   fn insert_char_reparses_via_core_epilogue_without_settle_call() {
       let mut e = crate::editor::Editor::new_from_text("# H\n", None, (80, 24));
       struct C; impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { 0 } }
       insert_char(&mut e, 'x', &C);
       // Core's resettle reparsed: blocks_version tracks the new version, caret visible.
       assert_eq!(e.active().reconcile.blocks_version, e.active().document.version);
       assert_eq!(e.active().document.buffer.to_string(), "x# H\n");
   }
   ```
   Run → passes both pre- and post-change (a regression guard, not a red→green). The behavioral proof
   of "no double" is `keystroke_runs_layout_once` staying green + the Surface-C fold tests (Task 5);
   the removed settle was a redundant epilogue that converged to the same final state (m-P1), so its
   removal is a cleanliness change verified by the green suite (§8 INV-EPILOGUE note).
2. **Implement — remove the redundant epilogue calls:**
   - **`commands/edit.rs` primitives + `commands/textops.rs`:** each currently ends `editor.apply(...);
     <return settle_after_edit(editor)>` or `... ; settle_after_edit(editor)`. Since `editor.apply`
     now runs `resettle`, replace the trailing `settle_after_edit(editor)` with `CommandResult::Handled`
     (the primitive's return). Do this ONLY on the arms that call `editor.apply` immediately before
     `settle_after_edit`; leave arms that settle after a NON-apply mutation (if any) untouched — verify
     each with `grep`/`cargo`. Example (`insert_char`, `commands/edit.rs:33–55`): the two
     `settle_after_edit(editor)` tails become `CommandResult::Handled`.
   - **`prose_ops::{move_sentence, break_paragraph_here, merge_paragraph_forward,
     split_sentence_at_caret}`:** each does `editor.apply(...); let r = super::edit::settle_after_edit(editor);
     editor.set_status(...); r`. Drop the `settle_after_edit` call: `editor.apply(...);
     editor.set_status(...); CommandResult::Handled`. (Keep `set_status`.)
   - **`blocks_marked::apply_edit` helper (`:80–91`):** delete its manual `crate::derive::rebuild(editor);
     nav::ensure_visible(editor); editor.active_mut().desired_col = None;` (`:89–91`) — the core does
     them. The helper becomes:
     ```rust
     fn apply_edit(editor: &mut Editor, cs: wordcartel_core::change::ChangeSet,
                   edit: wordcartel_core::block_tree::Edit, new_caret: usize, clock: &dyn Clock) {
         let txn = wordcartel_core::history::Transaction::new(cs)
             .with_selection(wordcartel_core::selection::Selection::single(new_caret));
         editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
     }
     ```
     (`block_copy`/`block_delete` are unaffected; `block_move`'s use is adjusted in Task 5. Remove the
     now-unused `use crate::nav;` in `blocks_marked.rs` ONLY if `cargo build` reports it unused.)
   - **`move_block_to_scratch` (`scratch.rs:57–61`):** the active source-delete `editor.apply(...)` now
     settles via the core; drop the explicit `crate::derive::rebuild(editor); crate::nav::ensure_visible(editor);`
     (`:59–60`). Keep `editor.active_mut().marked_block = None;` (`:58`) and the `set_status` (`:61`).
   - **`diag_apply_selected` (`search_ui.rs:206–213`):** the suggestion apply already uses
     `editor.apply(...)` (core-backed). Collapse the double rebuild:
     ```rust
       editor.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
       crate::registry::unfold_ancestors_of(editor, new_cursor);
       crate::edit_apply::resettle(editor); // reflect the unfold on the already-reparsed tree
       editor.diag = None;
     ```
     (Was: `editor.apply` → `derive::rebuild` → `unfold_ancestors_of` → `derive::rebuild` →
     `ensure_visible`. The first rebuild is now the core's; one `resettle` after the unfold suffices.)
3. **Run** `cargo test -p wordcartel commands blocks_marked scratch search derive` + full
   `cargo test -p wordcartel` → green. `cargo test -p wordcartel-core` → green. Clippy clean.
4. **Commit:** `H22 Task 4: drop redundant epilogues from standard Editor::apply clients (core owns resettle)`.

---

## Task 5 — Surface C: migrate `swap` + `block_move`, preserving the fold correction

**Deliverable:** both custom-fold commands route their apply through the core (rebuild #1); each keeps
its pre-apply `corrected_after_move` and its post-`replace_folded` rebuild #2 (via `resettle`, J5).

**Interfaces**
- *Consumes:* `edit_apply::resettle`; `Editor::apply`; `fold::corrected_after_move`;
  `FoldState::replace_folded`; `registry::snap_caret_out_of_fold`. Sites: `swap` (`prose_ops.rs:101–168`);
  `block_move` (`blocks_marked.rs:36–68`).
- *Ordering constraint:* `corrected_after_move` stays computed BEFORE `Transaction::new(cs)` / the
  apply — `prose_ops.rs:149–151` before `:153/:154`; `blocks_marked.rs:54–56` before `:57`.

**TDD steps**
1. **REGRESSION GUARDS — fold survival must not regress (green BEFORE and AFTER; not red-first).**
   Surface C is behavior-preserving (§3.6): these guards characterize the fold survival the migration
   must keep, so they pass on the baseline and must still pass after. Keep green the existing
   fold-survival batteries (`blocks_marked.rs` tests near `:265`; `prose_ops.rs` tests near `:519`).
   Add one focused pin per command asserting the post-migration corrected fold set for a folded-region
   move/swap equals the pre-migration expectation (grounded on the `corrected_after_move` fixtures
   `fold.rs:536–564`). Example for `block_move`:
   ```rust
   #[test]
   fn block_move_preserves_a_folded_region_through_the_core() {
       // A folded heading region moved past the caret keeps its fold (corrected_after_move) and the
       // caret is never left on a hidden line (snap_caret_out_of_fold).
       // (Reuse the existing block_move fold fixture shape; assert folds().contains(dest_anchor).)
   }
   ```
   Run BEFORE the code change → passes (baseline), and again AFTER (unchanged) — the regression
   tripwire for the migration.
2. **Implement:**
   - **`swap` (`prose_ops.rs:154–160`):** the `editor.apply(...)` at `:154` now runs the core epilogue
     (rebuild #1). DELETE the explicit rebuild #1 at `:157` and remove the obsolete comment; keep the
     `corrected` compute at `:149–151` (pre-apply — unchanged) and the post-`replace_folded` re-settle:
     ```rust
       let txn = Transaction::new(cs).with_selection(Selection::range(moved_to, moved_from));
       editor.apply(txn, edit, EditKind::Other, clock); // core: mutate + rebuild #1 + ensure_visible
       editor.active_mut().marked_block = None;
       if let Some(c) = corrected {
           editor.active_mut().folds.replace_folded(c); // override the core's plain remap with the corrected set
       }
       let r = super::edit::settle_after_edit(editor); // rebuild #2 — relayout + reconcile corrected folds
       if had_correction {
           crate::registry::snap_caret_out_of_fold(editor);
       }
       editor.set_status(crate::status::StatusKind::Info, "swapped");
       r
     ```
     (`settle_after_edit` still returns `CommandResult::Handled` → `r`. `corrected`/`had_correction`
     are computed at `:149–152`, before `Transaction::new` at `:153` — the ordering constraint holds.)
   - **`block_move` (`blocks_marked.rs:57–64`):** the `apply_edit` helper (now epilogue-free +
     core-backed from Task 4) runs rebuild #1. Repoint the bare rebuild #2 at `:60` to `resettle` (J5):
     ```rust
       apply_edit(editor, cs, edit, new_caret, clock); // core: mutate + rebuild #1 + ensure_visible
       if let Some(c) = corrected {
           editor.active_mut().folds.replace_folded(c);
           crate::edit_apply::resettle(editor);           // rebuild #2 — relayout + reconcile corrected folds
           crate::registry::snap_caret_out_of_fold(editor);
       }
       editor.active_mut().marked_block = None; // consumed
     ```
     (`corrected` is computed at `:54–56`, before `build_multi_replace`/`apply_edit` — ordering holds.)
3. **Run** the fold batteries + full `cargo test -p wordcartel` → green. Clippy clean.
4. **Commit:** `H22 Task 5: migrate swap + block_move (fold correction preserved, rebuild #2 via resettle)`.

---

## Task 6 — F6: `record_snapshot` in `Buffer::undo`/`redo`

**Deliverable:** undo/redo refresh the `LAST_GOOD` recovery latch so a panic dump can't resurrect
undone content.

**Interfaces**
- *Consumes:* `recovery::record_snapshot(path: Option<&Path>, rope: ropey::Rope)` (`recovery.rs:11`);
  `Buffer::undo` (`editor.rs:306`), `Buffer::redo` (`editor.rs:325`); `Document.path: Option<PathBuf>`;
  `TextBuffer::snapshot(&self) -> ropey::Rope` (`buffer.rs:166`); `recovery::LAST_GOOD:
  Mutex<Option<(Option<PathBuf>, ropey::Rope)>>` (`recovery.rs:8`).
- *Produces:* unchanged signatures.

**TDD steps**
1. **Failing test** — add to `editor.rs` tests:
   ```rust
   #[test]
   fn undo_and_redo_refresh_the_recovery_snapshot() {
       // Serialize the shared LAST_GOOD latch for this thread by taking it, seeding a sentinel,
       // dropping the guard, then acting. (Global; other threads may race — see plan note.)
       struct C; impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { 0 } }
       let mut e = Editor::new_from_text("abc\n", None, (80, 24));
       let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "X".into())], 4);
       e.active_mut().apply(Transaction::new(cs), edit, EditKind::Other, &C); // LAST_GOOD = "Xabc\n"
       e.active_mut().undo();
       let after_undo = crate::recovery::LAST_GOOD.lock().unwrap()
           .as_ref().map(|(_, r)| r.to_string());
       assert_eq!(after_undo.as_deref(), Some("abc\n"), "undo refreshes the recovery snapshot");
       e.active_mut().redo();
       let after_redo = crate::recovery::LAST_GOOD.lock().unwrap()
           .as_ref().map(|(_, r)| r.to_string());
       assert_eq!(after_redo.as_deref(), Some("Xabc\n"), "redo refreshes the recovery snapshot");
   }
   ```
   Run → **fails**: today `undo`/`redo` never call `record_snapshot`, so after `undo()` the latch
   still holds "Xabc\n".
2. **Implement** — in `Buffer::undo` (`editor.rs:308–321`, the `Some(sel)` arm) and `Buffer::redo`
   (`editor.rs:327–340`, the `Some(sel)` arm), add after `self.document.version += 1;`:
   ```rust
                   // F6: refresh the recovery panic-latch so a post-undo/redo dump can't resurrect
                   // content the user deliberately reverted (H22 — the one widening of "undo/redo
                   // stay OUT of the apply core"; apply already snapshots at editor.rs:304).
                   crate::recovery::record_snapshot(
                       self.document.path.as_deref(), self.document.buffer.snapshot());
   ```
3. **Run** `cargo test -p wordcartel undo_and_redo_refresh` → green; full `cargo test -p wordcartel`
   → green. Clippy clean.
4. **Commit:** `H22 Task 6: undo/redo refresh the recovery snapshot (F6 LAST_GOOD gap)`.

**Note (global-state test):** `LAST_GOOD` is process-global; a parallel test calling `apply`/`undo`
could interleave a `record_snapshot` between this test's action and its read. The window is tiny
(both are synchronous, same-thread reads immediately follow the op). If flake is observed, the plan's
fallback is `#[serial]`-style isolation via a dedicated single-threaded test binary — but do not add
that dependency speculatively.

---

## Task 7 — Demote `Buffer::apply` to `pub(crate)` + INV-SEAM scan + INV-LAZY-HEAL test

**Deliverable:** the compiler-guarded no-bypass seam lands (external crates blocked), the in-crate
scan tripwire is in place, and the lazy-heal invariant is pinned. This is LAST — it only compiles/
passes after every surface is migrated.

**Interfaces**
- *Consumes:* `Buffer::apply` (`editor.rs:267`, demote to `pub(crate)`); `production_lines` idea from
  `tests/module_budgets.rs:21`; `workspace::switch_to(editor, idx)` (`workspace.rs:52`);
  `edit_apply::apply_edit`.
- *Produces:* `Buffer::apply` is `pub(crate)`; new integration test `wordcartel/tests/edit_seam.rs`;
  INV-LAZY-HEAL unit test in `edit_apply.rs`.

**TDD steps**
1. **INV-LAZY-HEAL invariant pin (passes as soon as Task 1's active-gate exists — a characterization
   pin, not red-first).** This locks F3's lazy-heal behavior; it is green from Task 1 onward. Add it in
   the `edit_apply.rs` test module. It is the ONLY Task-test that needs `crate::editor::Buffer`, so it
   fully-qualifies it (Task 1's module `use` deliberately imports only `Editor`, m-P2):
   ```rust
   #[test]
   fn non_active_edit_lags_then_heals_on_switch() {
       let mut e = Editor::new_from_text("alpha\n", None, (80, 24));
       let id1 = e.alloc_id();
       let area = e.active().view.area;
       // buffer 0 stays active; edit lands on the non-active buffer id1.
       e.buffers.push(crate::editor::Buffer::from_text(id1, "beta\n", None, area));
       let doc_len = e.by_id(id1).unwrap().document.buffer.len();
       let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "X".into())], doc_len);
       let out = apply_edit(&mut e, id1, Transaction::new(cs), edit, EditKind::Other, &C(0));
       assert_eq!(out, EditOutcome::Applied);
       let b = e.by_id(id1).unwrap();
       assert!(b.reconcile.blocks_version < b.document.version,
           "non-active tree lags — the core skips the epilogue (INV-LAZY-HEAL)");
       // Every activation path heals before first render:
       let idx = e.buffers.iter().position(|x| x.id == id1).unwrap();
       crate::workspace::switch_to(&mut e, idx);
       let b = e.by_id(id1).unwrap();
       assert_eq!(b.reconcile.blocks_version, b.document.version, "switch_to heals the tree");
   }
   ```
   Run → green (Task 1's core already has the active-gate) — a characterization pin, green now and
   after the demotion.
2. **INV-SEAM scan test** — new file `wordcartel/tests/edit_seam.rs`:
   ```rust
   //! INV-SEAM (H22, J3): after the universal-edit-chokepoint migration, `Buffer::apply` is the sole
   //! raw mutation channel. Two guards: (1) `Buffer::apply` is `pub(crate)` — the COMPILER blocks
   //! every out-of-crate bypass (the Effort-P concern). (2) This heuristic source scan catches the
   //! COMMON in-crate regression: a raw `Buffer::apply` reached through an accessor CHAIN
   //! (`active_mut()`/`active()`/`by_id_mut(..)` immediately `.apply(`) anywhere in production.
   //! The sanctioned core writes it as a two-statement `let b = by_id_mut(..); b.apply(..)` pair, so
   //! it is NOT matched and needs no allowlist. Residual (documented, spec §8.1): a `let`-bound
   //! Buffer local elsewhere would evade the text scan — the `pub(crate)` compiler guard + review
   //! cover that. Heuristic by design, paired with the compiler.
   use std::path::{Path, PathBuf};

   /// Production region = source before a co-located `mod tests` (mirrors module_budgets.rs:21).
   fn production(src: &str) -> String {
       let lines: Vec<&str> = src.lines().collect();
       match lines.iter().rposition(|l| l.trim_start().starts_with("mod tests")) {
           Some(i) => lines[..i].join("\n"),
           None => src.to_string(),
       }
   }

   fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
       for e in std::fs::read_dir(dir).unwrap() {
           let p = e.unwrap().path();
           if p.is_dir() { rs_files(&p, out); }
           else if p.extension().and_then(|x| x.to_str()) == Some("rs") { out.push(p); }
       }
   }

   /// A line reaches a raw `Buffer::apply` through an accessor chain.
   fn is_chained_raw_apply(line: &str) -> bool {
       line.contains(".active_mut().apply(")
           || line.contains(".active().apply(")
           || (line.contains("by_id_mut(") && line.contains(".apply("))
   }

   #[test]
   fn buffer_apply_is_the_sole_raw_mutation_channel() {
       let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
       let mut files = Vec::new();
       rs_files(&src, &mut files);
       let mut offenders = Vec::new();
       for f in &files {
           let text = std::fs::read_to_string(f).unwrap();
           for (n, line) in production(&text).lines().enumerate() {
               if is_chained_raw_apply(line) {
                   offenders.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
               }
           }
       }
       assert!(offenders.is_empty(),
           "INV-SEAM: raw Buffer::apply reached through an accessor chain — route it through \
            edit_apply::apply_edit instead:\n{}", offenders.join("\n"));
   }

   #[test]
   fn buffer_apply_is_pub_crate_within_impl_buffer() {
       // C-P1: scope the visibility check to the `impl Buffer { … }` block so it cannot see
       // `Editor::apply` (editor.rs:1062, intentionally still `pub` and — after J2 — also
       // `fn apply(&mut self, txn:`). Slice from `impl Buffer {` to the NEXT top-level `impl `.
       let editor = std::fs::read_to_string(
           Path::new(env!("CARGO_MANIFEST_DIR")).join("src/editor.rs")).unwrap();
       let start = editor.find("\nimpl Buffer {").expect("impl Buffer block must exist");
       let rest = &editor[start + 1..];                       // drop the leading '\n'
       let end = rest[1..].find("\nimpl ").map(|i| i + 1).unwrap_or(rest.len());
       let impl_buffer = &rest[..end];                        // the `impl Buffer { … }` body only
       // Sanity: the slice really is impl Buffer and excludes impl Editor's apply.
       assert!(impl_buffer.starts_with("impl Buffer {"), "slice must start at impl Buffer");
       assert!(!impl_buffer.contains("impl Editor"), "slice must not reach impl Editor");
       assert!(impl_buffer.contains("pub(crate) fn apply(&mut self, txn:"),
           "INV-SEAM: Buffer::apply must be pub(crate) (compiler blocks out-of-crate bypass)");
       // A bare `pub fn apply(&mut self, txn:` inside impl Buffer would re-open the external bypass.
       // (`pub(crate) fn apply…` does NOT contain the substring `pub fn apply…`, so this is exact.)
       assert!(!impl_buffer.contains("pub fn apply(&mut self, txn:"),
           "INV-SEAM: Buffer::apply must NOT be bare `pub` — a widen re-opens the external bypass");
   }
   ```
   Run BEFORE the demotion → `buffer_apply_is_pub_crate_within_impl_buffer` **fails** (Buffer::apply is
   still `pub`, so the `pub(crate)` assertion is unmet); the chained-scan already passes (Tasks 3–4
   removed `scratch.rs:21` and the search `active_mut().apply` sites).
3. **Implement the demotion** — change `editor.rs:267` from `pub fn apply` to `pub(crate) fn apply`
   (the `Buffer::apply` method only; `Editor::apply` at `:1062` stays `pub`). Verify no out-of-crate
   caller exists (there is none — the shell binary and tests are in-crate).
4. **Run** `cargo test -p wordcartel` (incl. the new integration test) + `cargo test -p
   wordcartel-core` → green. Full `cargo build`/`cargo test --no-run` warning-free. `cargo clippy
   --workspace --all-targets` → clean. Confirm `module_budgets` + `backlog` tests green.
5. **Commit:** `H22 Task 7: demote Buffer::apply to pub(crate) + INV-SEAM scan + INV-LAZY-HEAL test`.

---

## Final verification (before declaring the branch done)
- `cargo test` green across all suites (`wordcartel-core` lib + oracle, `wordcartel` lib + integration).
- `cargo build` + `cargo test --no-run` warning-free (touched crates).
- `cargo clippy --workspace --all-targets` clean.
- `module_budgets` + `backlog` gates green; `edit_apply.rs` under the `too_many_lines` threshold.
- Run `scripts/smoke/run.sh`; quote its one-line summary in the pre-merge report (advisory-pass).
- Command-surface-contract statement (N/A-with-caveat) recorded in the final report.
- Whole-branch Fable review + Codex pre-merge GO (both gates) before merge.

---

## Self-review (writing-plans conventions)

- **Spec coverage:** all four surfaces are tasked — A (Task 3, 7 sites), B (Task 4, edit.rs +
  textops.rs + prose_ops 91/197/250/293 + blocks_marked helper + move_block_to_scratch:57 +
  diag_apply_selected:208), S (Task 2, submit_transaction; plugin api.rs untouched), C (Task 5,
  swap + block_move with the ordering constraint). All six resolved decisions are baked in: F1=B
  (Task 1 new module), F2=A (Task 1 `resettle` relocation; Tasks 4–5 drop redundant epilogues),
  F3=B (Task 7 INV-LAZY-HEAL test + the core's active-gate), F4=A (Task 1 guard + `EditOutcome`; J2
  widen; Task 3 false-ack fix), F5=A (Task 1 `debug_assert!` on `validate_against(&b.document.buffer)`),
  F6=A (Task 6), J3 (Task 7 scan), J5 (Task 5 `block_move` rebuild #2 via `resettle`). All six invariant
  tests present: INV-SEAM (Task 7 scan + pub(crate)), INV-LAZY-HEAL (Task 7), INV-GUARD false-ack
  (Task 3), INV-EPILOGUE (Task 4 guard + kept `keystroke_runs_layout_once`), F6 snapshot (Task 6),
  Surface-C fold survival (Task 5).
- **Placeholder scan:** no TODO/`unimplemented!()`/`...` in any code block; every snippet is complete
  and compiles against the cited signatures.
- **Type consistency:** `apply_edit` returns `EditOutcome` (matched exhaustively at call sites);
  `Editor::apply` widened `()`→`EditOutcome` (statement-position callers unaffected);
  `settle_after_edit` keeps `-> CommandResult`; `resettle` is `pub(crate) -> ()`;
  `record_snapshot(Option<&Path>, ropey::Rope)` args verified (`path.as_deref()`, `buffer.snapshot()`);
  `validate_against(&TextBuffer)` fed `&b.document.buffer` (C1). `Transaction.changes` is `pub`
  (`history.rs:54`) so the debug_assert compiles.
- **Plan-gate round-1 folds:** C-P1 (INV-SEAM visibility test now slices the `impl Buffer` block so it
  cannot see `Editor::apply` — verified against `editor.rs`'s three impls at 72/196/601); I-P1 (Task 2
  relabelled a regression guard — the original delegator was already loud, so there is no false red);
  I-P2 (Task 1 red step now adds `pub mod edit_apply;` + a compiling stub so the red is on the
  ASSERTION); I-P3 (Task 4 epilogue guard, Task 5 fold guards, Task 7 lazy-heal pin relabelled
  green-before-and-after regression/characterization guards); m-P1 ("gated no-op" → "converges to
  identical final state; second `ensure_visible` re-scrolls idempotently"); m-P2 (Task 1 imports only
  `Editor`; the lazy-heal test fully-qualifies `crate::editor::Buffer`).
- **Sequencing judgment calls (below).**

## Sequencing judgment calls (for the human / Codex plan-gate)
1. **`Editor::apply` widened in Task 1, surfaces cleaned in Tasks 3–5.** Between Task 1 and those,
   migrated clients run the epilogue TWICE (core `resettle` + their own `settle_after_edit`/rebuild).
   The double run CONVERGES to an identical final state (rebuild is LayoutKey-gated; the second
   `ensure_visible` re-scrolls idempotently — m-P1), so every intermediate commit is green — but
   reviewers should expect the transient "double" to exist. Alternative (widen `Editor::apply` last)
   would force all surfaces to call `edit_apply::apply_edit` directly and churn ~40 active-buffer sites
   off the ergonomic `editor.apply`; rejected as more churn for no correctness gain.
2. **INV-SEAM scan is heuristic (chained-accessor + `pub(crate)` grep), not a type-accurate check**
   (J3, unavoidable: a raw `buffer.apply(...)` and `editor.apply(...)` are textually identical at the
   call site — grep can't see the receiver type). The compiler covers the external/plugin boundary
   that actually matters; the scan catches the common in-crate regression form. A `let`-bound Buffer
   local elsewhere evades it — documented in the test. If you want type-accurate enforcement, the only
   option is co-locating the core into `editor.rs` (rejects F1=B) — flagged, not taken.
3. **F6 test reads a process-global (`LAST_GOOD`).** Minor parallel-race window; noted with a
   `#[serial]` fallback that is deliberately NOT added speculatively (no new dep unless flake appears).
