# H1 app.rs Decomposition Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** split `wordcartel/src/app.rs` (5,740 lines) into four new modules (`jobs_apply.rs`, `session_restore.rs`, `prompts.rs`, `search_ui.rs`) plus a residual app.rs (Msg + overlay glue + reduce + run loop) — a purely mechanical, behavior-preserving move.

**Architecture:** verbatim cut-and-paste along the spec's seams, one commit per module in dependency order, full gates green after every commit. NO logic changes anywhere: the only permitted line-level changes are the spec's clauses (a) `use` headers, (b) enumerated visibility keywords, (c) cross-module call paths, (d) module doc-comments. Tests move by the spec's two-pronged rule (24 move, 115 stay; stayers get path-prefix swaps only).

**Tech Stack:** Rust; no new dependencies; no API changes visible outside the crate (main.rs untouched).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-04-wordcartel-h1-app-decomposition-design.md` (CLEAN — Codex ×3 + Fable ×3). Its visibility contract and clause (a)-(d) list govern every task. **Any changed line not covered by a clause is a defect.**
- **Gates after EVERY commit:** `cargo test -p wordcartel-core -p wordcartel` green (975 tests), `cargo clippy --workspace --all-targets` clean (deny gate LIVE), `cargo build` warning-free. NO `cargo fmt` — the repo is hand-formatted; moved code keeps its bytes.
- **Line numbers in this plan are BRANCH-BASE references** (the file state when the task begins for Task 1; earlier tasks shift later numbers). Locate every cut by the quoted signature/doc text and verify the extent (line count) matches; do the cuts within a task from the BOTTOM of the file upward so the listed numbers stay valid while you work. After cuts, make path edits by string match, not line number.
- Moved test bodies call `crate::app::<fn>` today (grep-verified, no bare-name calls). A MOVING test rewrites those calls to bare names under its new `use super::*`; a STAYING test swaps only the module segment (`crate::app::` → `crate::prompts::` etc.). No other test edits of any kind — never weaken, rename, or reorder a test.
- House style: `—` em-dash in prose comments; no emoji; match neighbors by hand.
- Every commit message ends with the trailers, verbatim (use `git commit -F -` with a quoted heredoc — `!` breaks zsh inside double-quoted `-m`):
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: `jobs_apply.rs` — job/message application

**Files:**
- Create: `wordcartel/src/jobs_apply.rs`
- Modify: `wordcartel/src/lib.rs` (one `mod` line), `wordcartel/src/app.rs` (cuts + path edits), `wordcartel/src/save.rs`, `wordcartel/src/reconcile.rs`, `wordcartel/src/swap.rs`, `wordcartel/src/file.rs` (external `apply_outcome` renames)

**Interfaces:**
- Consumes: `crate::app::Msg` (stays in app.rs — do NOT move it).
- Produces: `crate::jobs_apply::{apply_result, apply_job_result, apply_outcome, apply_job_outcome, drive_quit_drain}` (`pub`) and `{apply_filter_done, apply_transform_done, apply_export_done, insert_paste_text, apply_clipboard_paste, apply_clipboard_availability}` (`pub(crate)`) — Task 3's `resolve_prompt` calls `crate::jobs_apply::drive_quit_drain`.

- [ ] **Step 1: create the module and declare it.** New file `wordcartel/src/jobs_apply.rs` starting exactly:

```rust
//! Job/message application: merging finished job results and outcomes into the editor,
//! quit-drain advancement, and the async filter/transform/export/clipboard completion
//! handlers. Extracted verbatim from app.rs (Effort H1).

use crate::{editor::Editor, file};
use crate::jobs::{is_stale, Executor, JobResult};
use crate::registry::Ctx;
use crate::app::Msg;
use wordcartel_core::history::Clock;
```

(The bodies reach everything else via full paths — `crate::jobs::JobOutcome`, `crate::workspace::*`, `crate::save::*`, `wordcartel_core::*`, `std::*` — which need no `use`. `apply_panic` carries its own in-body `use crate::jobs::JobKind;` that moves verbatim with it.) In `wordcartel/src/lib.rs`, add `mod jobs_apply;` on its own line immediately after the existing `mod jobs;` declaration (match the surrounding grouping style exactly).

- [ ] **Step 2: cut the production code (bottom-up).** Two cuts from app.rs, pasted into jobs_apply.rs BELOW the use block, in source order (117-block first, clipboard trio after it):
  1. Cut app.rs **:769-:813** (45 lines) — `insert_paste_text` (no doc), `apply_clipboard_paste`, `apply_clipboard_availability`.
  2. Cut app.rs **:117-:403** (287 lines) — from the `///` doc line above `pub fn apply_result` (:117) through the closing brace of `apply_export_done` (:403): `apply_result`, `apply_job_result` (doc :177-:179), `apply_outcome` (doc :188-:189), `apply_panic`, `apply_job_outcome` (doc :233-:235), `drive_quit_drain` (doc :244-:247), `apply_filter_done` (`//` comment :284-:285 + `#[allow(clippy::too_many_arguments)]` :286 — both move), `apply_transform_done`, `apply_export_done`.

- [ ] **Step 3: apply the six visibility escalations** (the ONLY keyword edits in this task): `apply_filter_done`, `apply_transform_done`, `apply_export_done`, `insert_paste_text`, `apply_clipboard_paste`, `apply_clipboard_availability` each change `fn` → `pub(crate) fn`. `apply_panic` STAYS private; the five `pub` fns stay `pub`.

- [ ] **Step 4: residual app.rs path edits** (string-match; all bare calls today gain the `crate::jobs_apply::` prefix — full-paths-inline is the house style, add NO `use` line):
  - `apply_job_outcome(` at :868 (inside `dispatch_overlay_command`, which stays) and at reduce's 24 sites :1151 :1164 :1168 :1201 :1214 :1225 :1338 :1352 :1363 :1450 :1462 :1472 :1587 :1621 :1640 :1688 :1714 :1725 :1750 :1771 :1836 :1876 :1954 :2009.
  - `apply_filter_done(` :1623 :1956; `apply_export_done(` :1626 :1959; `apply_transform_done(` :1629 :1962; `apply_clipboard_paste(` :1634 :1993; `apply_clipboard_availability(` :1635 :1994; `insert_paste_text(` :1934.
  - **The clause-(c) transient:** inside `resolve_prompt` (still in app.rs until Task 3), `drive_quit_drain(` at :641 and :653 → `crate::jobs_apply::drive_quit_drain(`. This line moves again with `resolve_prompt` in Task 3 — expected and spec-sanctioned.

- [ ] **Step 5: external call-site renames.** `crate::app::apply_outcome` → `crate::jobs_apply::apply_outcome` at exactly 14 sites: save.rs:329 :348 :369 :393 :424 :433 :487 :770, reconcile.rs:120 :151 :176, swap.rs:567 :583, file.rs:315. Verify with `grep -rn "crate::app::apply_outcome" wordcartel/src/` → zero remaining.

- [ ] **Step 6: move the seven tests** into a new `#[cfg(test)] mod tests { use super::*; … }` at the bottom of jobs_apply.rs. Cut ranges (bottom-up; each includes its preceding doc/`#[test]`): :4046-:4058 `durability_result_for_missing_buffer_still_runs`; :3379-:3391 `buffer_local_result_for_live_buffer_merges`; :3365-:3377 `buffer_local_result_for_missing_buffer_is_dropped`; :3348-:3363 `apply_result_merges_fresh_and_drops_stale`; :3306-:3346 `quit_after_save_cancelled_when_edited_during_flight`; :3285-:3304 `save_and_quit_arms_pending_after_save_quit_and_exits`; :3243-:3266 `save_and_quit_command_arms_pending_after_save_like_prompt`. (CAUTION: the STAYING test `save_and_quit_command_on_unnamed_buffer_does_not_arm` sits at :3268-:3283 BETWEEN two of these cuts — do not take it.) In the moved bodies, rewrite `crate::app::apply_result` / `crate::app::apply_outcome` calls to bare `apply_result` / `apply_outcome` (via `use super::*`); every other path in those bodies stays as written. None of the seven uses any app.rs test helper (grounding-verified).

- [ ] **Step 7: staying-test segment swaps for THIS task's fns only:** `crate::app::apply_job_outcome` → `crate::jobs_apply::apply_job_outcome` at :3093 :3175; `crate::app::apply_outcome` → `crate::jobs_apply::apply_outcome` at :3221 :5093. (Their `crate::app::resolve_prompt`/`save_as_submit` calls are Task 3's swaps — leave them.)

- [ ] **Step 8: gates.** Run `cargo test -p wordcartel-core -p wordcartel` (expect 975 passed, 0 failed — same totals, tests relocated not changed), `cargo clippy --workspace --all-targets` (clean), `cargo build` (warning-free).

- [ ] **Step 9: commit** (heredoc trailers per Global Constraints): `refactor(h1): extract jobs_apply.rs from app.rs — verbatim move, 12 fns + 7 tests`.

---

### Task 2: `session_restore.rs` — session/resume restoration

**Files:**
- Create: `wordcartel/src/session_restore.rs`
- Modify: `wordcartel/src/lib.rs`, `wordcartel/src/app.rs`, `wordcartel/src/workspace.rs`

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `crate::session_restore::{apply_resume, load_marks_from_entry, load_block_from_entry, restore_resume, restore_scratch, open_into_current}` (all `pub`, unchanged visibility).

- [ ] **Step 1: create + declare.** New file `wordcartel/src/session_restore.rs` starting exactly:

```rust
//! Session/resume restoration: cursor/scroll/marks/folds/block restore and the
//! open-into-current buffer-load seam. Extracted verbatim from app.rs (Effort H1).

use crate::editor::Editor;
```

(Only `Editor` appears bare; everything else is full-path in the bodies.) In lib.rs, add `mod session_restore;` beside the state/session-related declarations — immediately after the existing `mod state;` line (match grouping).

- [ ] **Step 2: cut the production span.** One contiguous cut, branch-base app.rs **:405-:528** (124 lines; after Task 1 it sits ~287 lines higher — locate by the `///` doc above `pub fn apply_resume`): `apply_resume` (doc :405-:406), `load_marks_from_entry` (doc :418-:420), `load_block_from_entry` (doc :430-:440), `restore_resume` (doc :451-:454), `restore_scratch` (doc :477-:480), `open_into_current` (doc :501-:504). Paste below the use block. ZERO visibility edits in this task.

- [ ] **Step 3: path edits.** Residual app.rs (both inside `run()`): `restore_scratch(` :2145 and `restore_resume(` :2295 gain `crate::session_restore::`. The in-module `open_into_current → restore_resume` call stays bare (both moved together). External: workspace.rs:75 `crate::app::open_into_current` → `crate::session_restore::open_into_current`; workspace.rs:85 `crate::app::restore_resume` → `crate::session_restore::restore_resume`.

- [ ] **Step 4: move the seven tests** (bottom-up; new `#[cfg(test)] mod tests { use super::*; … }`): :5213-:5223 `restore_scratch_loads_text_and_clamps_cursor`; :4314-:4362 `restore_clamps_out_of_range_block_no_slice_panic` (doc :4314-:4318); :4257-:4312 `marked_block_persists_and_restores_under_matching_identity` (doc :4257-:4259); :4243-:4255 `load_marks_from_entry_populates_clamped`; :4143-:4154 `resume_restores_when_identity_matches_and_clamps_when_not`; :2584-:2596 `file_browser_enter_on_file_opens_it_when_clean`; :2570-:2582 `open_into_current_replaces_with_fresh_id_and_clean`. (CAUTION: the `f10()` helper at :2598-:2605 immediately follows the :2584 cut and STAYS in app.rs.) Rewrite the movers' `crate::app::{open_into_current, apply_resume, load_marks_from_entry, load_block_from_entry, restore_scratch}` calls to bare names. No staying test calls a session_restore fn — no segment swaps in this task.

- [ ] **Step 5: gates** (same three commands, same expectations).

- [ ] **Step 6: commit:** `refactor(h1): extract session_restore.rs from app.rs — verbatim move, 6 fns + 7 tests`.

---

### Task 3: `prompts.rs` — prompt submits & file dialogs

**Files:**
- Create: `wordcartel/src/prompts.rs`
- Modify: `wordcartel/src/lib.rs`, `wordcartel/src/app.rs`, `wordcartel/src/registry.rs`, `wordcartel/src/save.rs`

**Interfaces:**
- Consumes: `crate::jobs_apply::drive_quit_drain` (Task 1) — already path-qualified inside `resolve_prompt` by Task 1's clause-(c) edit, so the fn body moves as-is here.
- Produces: `crate::prompts::{open_save_as, expand_path, save_as_submit, block_write_submit, request_new, resolve_prompt}` (`pub`), `{submit_filter_line, goto_line_submit}` (`pub(crate)`); `perform_block_write`/`perform_save_as` stay private inside the module.

- [ ] **Step 1: create + declare.** New file `wordcartel/src/prompts.rs` starting exactly:

```rust
//! Prompt submits & file dialogs: Save-As / Write-Block / New / go-to-line, and the
//! modal PromptAction resolver. Extracted verbatim from app.rs (Effort H1).

use crate::editor::Editor;
use crate::jobs::Executor;
use crate::registry::Ctx;
use crate::prompt::PromptAction;
use crate::app::Msg;
use wordcartel_core::history::Clock;
```

In lib.rs, add `mod prompts;` immediately after the existing `mod prompt;` line.

- [ ] **Step 2: cut the production span.** One contiguous cut, branch-base app.rs **:530-:767** (238 lines — locate by the doc line `/// Execute the action chosen in a modal prompt…` at :530; NOTE this stray doc semantically describes `resolve_prompt` but is physically attached to `open_save_as` — it moves verbatim, do NOT relocate or reword it): `open_save_as` (doc :530-:531), `expand_path` (doc :539-:540), `save_as_submit` (doc :549-:550), `block_write_submit` (doc :572-:574), `perform_block_write`, `perform_save_as`, `request_new` (doc :608), `resolve_prompt` (no doc; its :641/:653 `crate::jobs_apply::drive_quit_drain` calls were rewritten in Task 1 and move as-is), `submit_filter_line` (doc :723-:727), `goto_line_submit` (doc :749-:750).

- [ ] **Step 3: the single visibility escalation:** `submit_filter_line` changes `fn` → `pub(crate) fn`. (`goto_line_submit` is already `pub(crate)`; `perform_block_write`/`perform_save_as` stay private — all their callers moved with them.)

- [ ] **Step 4: path edits.** Residual app.rs (reduce's modal + minibuffer-Enter arms): `resolve_prompt(` :1616, `submit_filter_line(` :1679, `goto_line_submit(` :1680, `save_as_submit(` :1681, `block_write_submit(` :1682 gain `crate::prompts::`. External: registry.rs:146 `crate::app::request_new` → `crate::prompts::request_new`; registry.rs:159 and save.rs:144 `crate::app::open_save_as` → `crate::prompts::open_save_as`.

- [ ] **Step 5: move the ten tests** (bottom-up): :5197-:5207 `block_write_existing_target_raises_overwrite`; :5182-:5195 `block_write_writes_block_text_only_doc_unchanged`; :5159-:5176 `new_additive_preserves_all_existing_buffers`; :5146-:5157 `new_on_dirty_buffer_is_additive_no_modal`; :5133-:5144 `new_on_any_buffer_adds_empty_untitled`; :3574-:3586 `goto_line_clamps_and_rejects_garbage`; :3554-:3572 `goto_line_into_folded_body_unfolds_to_reveal_target`; :3491-:3510 `recover_loads_body_and_deletes_orphan_swap_file`; :3226-:3241 `save_and_quit_on_unnamed_buffer_does_not_arm_pending_after_save`. Rewrite the movers' `crate::app::{resolve_prompt, save_as_submit, block_write_submit, request_new, goto_line_submit}` calls to bare names. None uses an app.rs test helper (grounding-verified).

- [ ] **Step 6: staying-test segment swaps for prompts fns:** `crate::app::resolve_prompt` → `crate::prompts::resolve_prompt` at :3087 :3114 :3117 :3139 :3174 :3196 :3218; `crate::app::save_as_submit` → `crate::prompts::save_as_submit` at :3201 :5092. Verify with `grep -n "crate::app::resolve_prompt\|crate::app::save_as_submit" wordcartel/src/app.rs` → zero remaining.

- [ ] **Step 7: gates.**

- [ ] **Step 8: commit:** `refactor(h1): extract prompts.rs from app.rs — verbatim move, 10 fns + 10 tests`.

---

### Task 4: `search_ui.rs` — search-and-replace + quick-fix UI

**Files:**
- Create: `wordcartel/src/search_ui.rs`
- Modify: `wordcartel/src/lib.rs`, `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `crate::search_ui::{search_sync, search_step, search_cancel, search_replace_all, search_step_apply, search_step_skip, search_step_rest, diag_apply_selected}` (all `pub(crate)`); `search_pin` and `type SearchReplacePlan` stay private inside the module.

- [ ] **Step 1: create + declare.** New file `wordcartel/src/search_ui.rs` starting exactly:

```rust
//! Search-and-replace + quick-fix (diagnostics) overlay actions. Extracted verbatim
//! from app.rs (Effort H1).

use crate::{derive, editor::Editor};
```

In lib.rs, add `mod search_ui;` immediately after the existing `mod search_overlay;` line (or, if declarations sit elsewhere, beside `mod search;` — match the file's grouping).

- [ ] **Step 2: cut the production span.** One contiguous cut, branch-base app.rs **:887-:1092** (206 lines — locate by `fn search_sync`; the span ends at `diag_apply_selected`'s closing brace, :1092; the overlay glue at :819-:885 ABOVE and `preview_selected_theme` at :1094 BELOW both STAY): `search_sync`, `search_step`, `search_cancel`, `type SearchReplacePlan` (:916), `search_replace_all`, `search_step_apply`, `search_step_skip`, `search_step_rest`, `search_pin`, `diag_apply_selected` (doc :1012-:1013).

- [ ] **Step 3: the eight visibility escalations:** `search_sync`, `search_step`, `search_cancel`, `search_replace_all`, `search_step_apply`, `search_step_skip`, `search_step_rest`, `diag_apply_selected` each change `fn` → `pub(crate) fn`. `search_pin` and `SearchReplacePlan` STAY private (all callers moved together).

- [ ] **Step 4: path edits.** Residual app.rs only (no external callers exist): `search_step_apply(` :1708, `search_step_skip(` :1709, `search_step_rest(` :1710, `search_cancel(` :1718, `search_replace_all(` :1721, `search_sync(` :1724 :1748, `search_step(` :1728 :1729 :1730 :1731, `diag_apply_selected(` :1767 gain `crate::search_ui::`.

- [ ] **Step 5: no test moves.** Zero tests direct-call any search_ui fn (all search/replace/quick-fix tests are reducer tests and stay). search_ui.rs ships with NO `#[cfg(test)]` block — deliberate; its coverage rides on the residual reducer suite. Do not write new tests (spec: "no new tests are required").

- [ ] **Step 6: gates.**

- [ ] **Step 7: commit:** `refactor(h1): extract search_ui.rs from app.rs — verbatim move, 10 items, 0 test moves (all reducer-driven)`.

---

### Task 5: polish — stale comments, header, bookkeeping

**Files:**
- Modify: `wordcartel/src/app.rs`, `wordcartel/src/editor.rs`, `wordcartel/src/workspace.rs`, `wordcartel/src/transform.rs`, `wordcartel/src/reconcile.rs`, `wordcartel/src/save.rs`, `docs/ux-backlog.md`

**Interfaces:** none — comment-only code edits plus doc bookkeeping.

- [ ] **Step 1: update the enumerated stale comments** (the LIST is the operative checklist — do not sweep for more; comments merely naming a moved fn without an "in app.rs" location claim need no edit). At each site, keep the sentence's wording and only correct the fn's stated home to its new module (branch-base line numbers; locate by content):
  - editor.rs:26 (`drive_quit_drain` — now `jobs_apply.rs`), :34, :363, :418, :429 (`apply_result`/`open_into_current` homes), :430 (`apply_job_result`), :642 (`diag_apply_selected` — now `search_ui.rs`)
  - workspace.rs:206, :216 (`open_into_current` — now `session_restore.rs`)
  - transform.rs:99 (`resolve_prompt` — now `prompts.rs`), :139 (`apply_transform_done` — now `jobs_apply.rs`)
  - reconcile.rs:130, :132 (`apply_panic` — now `jobs_apply.rs`)
  - save.rs:61 (`perform_save_as` — now `prompts.rs`)
  - app.rs:23 — the residual file's section-header comment (today it names "Msg, apply_result, reduce"): reword to describe what actually remains, e.g. `// Msg, the overlay glue, reduce, and the run loop — job/session/prompt/search handlers live in jobs_apply / session_restore / prompts / search_ui.` (match the header's existing comment style).
- [ ] **Step 2: verify lib.rs ordering** — the four new `mod` lines sit in the file's existing logical grouping (from Tasks 1-4); adjust placement only if a line landed outside its group.
- [ ] **Step 3: backlog bookkeeping** in `docs/ux-backlog.md`: mark H1 `SHIPPED` (date + residual line counts from `wc -l wordcartel/src/app.rs wordcartel/src/jobs_apply.rs wordcartel/src/session_restore.rs wordcartel/src/prompts.rs wordcartel/src/search_ui.rs`), and correct the H1 entry's `git log --follow` claim to `git blame -C -C` (a split is a copy, not a rename).
- [ ] **Step 4: gates** (unchanged expectations — comment-only code edits).
- [ ] **Step 5: commit:** `refactor(h1): polish — stale fn-home comments, residual header, backlog SHIPPED`.

---

## Verification appendix (for the final whole-branch review)

- Per-commit verbatim check: for each of commits 1-4, diff that commit against its parent and confirm every hunk is covered by clauses (a)-(d) — moved bodies byte-identical (compare the new file's fn bodies against the parent's app.rs ranges), visibility edits only those enumerated, path edits only those enumerated. The clause-(c) transient (`:641`/`:653` edited in commit 1, moved in commit 3) is expected.
- Totals: 24 tests moved (7 jobs_apply + 7 session_restore + 10 prompts + 0 search_ui), 115 stay; suite total stays 975 with 0 failures at every commit.
- `grep -rn "crate::app::\(apply_outcome\|apply_result\|apply_job\|drive_quit_drain\|open_into_current\|restore_\|resolve_prompt\|save_as_submit\|block_write_submit\|request_new\|goto_line_submit\|submit_filter_line\|search_\|diag_apply_selected\|open_save_as\|expand_path\|load_marks\|load_block\|apply_resume\|insert_paste_text\|apply_clipboard\|apply_filter_done\|apply_transform_done\|apply_export_done\)" wordcartel/src/` → zero hits after Task 4.
- Pre-merge: `scripts/smoke/run.sh` summary quoted verbatim (advisory).
