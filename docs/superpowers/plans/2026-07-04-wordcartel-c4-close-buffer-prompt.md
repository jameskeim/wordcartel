# C4 Close-Buffer Prompt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** closing a dirty buffer raises a Save/Discard/Cancel prompt (reusing the quit machinery's save-then-act path) instead of today's refusal.

**Architecture:** id-carrying `PromptAction::CloseSave/CloseDiscard { id }` + `PostSaveAction::CloseBuffer { id }` ride the existing `dispatch_save_then`/`pending_after_save` flow; a busy guard isolates the close prompt from other flows' pending state; per-case by-id `close_buffer_now` mechanics; quit supersedes-and-cancels a pending close. Task boundaries follow the compile-forcing analysis: the two exhaustive matches (prompts.rs:106, jobs_apply.rs:33) put the whole state machine in Task 1; the two `matches!`-only sites (timeout, quit-clear) are Task 2; e2e is Task 3.

**Tech Stack:** Rust; shell crate only (`wordcartel`); no new dependencies; no core changes.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-04-wordcartel-c4-close-buffer-prompt-design.md` (CLEAN — Codex ×3 + Fable ×2; three user-ratified decisions: Discard leaves the swap; NO keybinding; quit supersedes-and-cancels). D1-D4 govern.
- **Gates after EVERY commit:** `cargo test -p wordcartel-core -p wordcartel` green, `cargo clippy --workspace --all-targets` clean (deny gate LIVE), `cargo build` warning-free. NO `cargo fmt`; `—` em-dash prose; hand-match neighbors.
- **`PromptAction` MUST stay `Copy`** — `action_for` copies the action out of `&Choice` (prompt.rs:49); `BufferId` is `Copy` so the id-carrying variants preserve it. Do not touch the derive list.
- **One sanctioned pin reversal:** `close_refuses_dirty_buffer` (workspace.rs:316-329) becomes `close_dirty_raises_prompt` — the refusal it pins is what C4 removes. NO other existing test changes meaning.
- Line anchors are HEAD (`dff63ef`) references; locate by quoted code after earlier tasks shift lines.
- Every commit message ends with the trailers, verbatim (use `git commit -F -` with a quoted heredoc — `!` breaks zsh inside double-quoted `-m`):
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: the close state machine (ONE commit — the compile-forcing unit)

**Files:**
- Modify: `wordcartel/src/prompt.rs` (variants + constructor + routing test), `wordcartel/src/editor.rs` (one variant), `wordcartel/src/workspace.rs` (close_buffer + close_buffer_now + tests), `wordcartel/src/prompts.rs` (two resolve arms + tests), `wordcartel/src/jobs_apply.rs` (the CloseBuffer apply arm + tests)

**Interfaces:**
- Consumes: everything existing — `dispatch_save_then`, `buffer_display_name`, `open_prompt`, the InlineExecutor test idioms.
- Produces: `PromptAction::CloseSave { id } / CloseDiscard { id }`, `PostSaveAction::CloseBuffer { id }`, `pub(crate) workspace::close_buffer_now(editor, id)` — Task 2's timeout/quit-clear and Task 3's journeys consume these.

**Why one commit:** `resolve_prompt`'s match (prompts.rs:106) and `apply_result`'s match (jobs_apply.rs:33) are exhaustive — adding either enum variant forces those arms in the same compilation unit, and the arms call `close_buffer_now`. TDD stages are sequenced inside the task.

- [ ] **Step 1: the sanctioned pin reversal, RED.** In workspace.rs, rewrite `close_refuses_dirty_buffer` (:316-329) as (keeping its dirty-making idiom verbatim — the `build_multi_replace` + `Transaction` + `by_id_mut(aid).apply(...)` lines are unchanged):

```rust
    #[test]
    fn close_dirty_raises_prompt() {
        use wordcartel_core::history::Clock;
        struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let mut e = Editor::new_from_text("x\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch();
        let aid = e.active().id;
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "z".into())], 2);
        let txn = wordcartel_core::history::Transaction::new(cs).with_selection(wordcartel_core::selection::Selection::single(1));
        e.by_id_mut(aid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
        close_buffer(&mut e);
        assert!(e.by_id(aid).is_some(), "dirty buffer not closed by the prompt raise");
        let p = e.prompt.as_ref().expect("close-confirm prompt raised");
        assert_eq!(p.action_for('s'), Some(crate::prompt::PromptAction::CloseSave { id: aid }));
        assert_eq!(p.action_for('d'), Some(crate::prompt::PromptAction::CloseDiscard { id: aid }));
        assert_eq!(p.action_for('c'), Some(crate::prompt::PromptAction::Cancel));
    }
```

Also add the busy-guard test beside it:

```rust
    #[test]
    fn close_dirty_refuses_while_flow_pending() {
        use wordcartel_core::history::Clock;
        struct C(u64); impl Clock for C { fn now_ms(&self) -> u64 { self.0 } }
        let mut e = Editor::new_from_text("x\n", Some(std::path::PathBuf::from("/tmp/a.md")), (40, 10));
        e.install_scratch();
        let aid = e.active().id;
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "z".into())], 2);
        let txn = wordcartel_core::history::Transaction::new(cs).with_selection(wordcartel_core::selection::Selection::single(1));
        e.by_id_mut(aid).unwrap().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C(0));
        e.pending_after_save = Some(crate::editor::PendingAfterSave {
            buffer_id: aid, version: 1, action: crate::editor::PostSaveAction::Quit, at_ms: 0,
        });
        close_buffer(&mut e);
        assert!(e.prompt.is_none(), "busy guard: no prompt over pending state");
        assert!(e.status.contains("in progress"), "refusal status set: {:?}", e.status);
    }
```

Run: `cargo test -p wordcartel close_dirty` — FAIL to COMPILE (the variants don't exist yet); that is this stage's RED (record it).

- [ ] **Step 2: the enum additions + constructor.** prompt.rs — append to `PromptAction` (:5-27), preserving the derive list untouched:

```rust
    /// C4 close-buffer: save the target, then close it. The id is captured at
    /// raise time — background results can switch the active buffer under the
    /// prompt, so resolve must never read active().
    CloseSave { id: crate::editor::BufferId },
    /// C4 close-buffer: close the target without saving (the swap survives).
    CloseDiscard { id: crate::editor::BufferId },
```

Add the constructor before the impl block's end (:150), style-matching `quit_review_buffer` (:75-85):

```rust
    /// C4 close-confirm, raised when closing a dirty buffer (spec D1).
    pub fn close_confirm(name: &str, id: crate::editor::BufferId) -> Prompt {
        Prompt {
            message: format!("close {name}: [S]ave & close · [D]iscard · [C]ancel"),
            choices: vec![
                Choice { key: 's', label: "Save & close", action: PromptAction::CloseSave { id } },
                Choice { key: 'd', label: "Discard",      action: PromptAction::CloseDiscard { id } },
                Choice { key: 'c', label: "Cancel",       action: PromptAction::Cancel },
            ],
        }
    }
```

editor.rs — `PostSaveAction` (:16-17) becomes:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostSaveAction { Quit, ContinueQuitDrain, CloseBuffer { id: BufferId } }
```

And the routing test in prompt.rs's test module (sibling idiom :163-170):

```rust
    #[test]
    fn close_confirm_routes_keys_case_insensitively() {
        let id = crate::editor::BufferId(7);
        let p = Prompt::close_confirm("*a.md", id);
        assert_eq!(p.action_for('S'), Some(PromptAction::CloseSave { id }));
        assert_eq!(p.action_for('d'), Some(PromptAction::CloseDiscard { id }));
        assert_eq!(p.action_for('C'), Some(PromptAction::Cancel));
        assert_eq!(p.action_for('x'), None);
    }
```

(If `BufferId`'s constructor is not a public tuple — check editor.rs:10-11 — obtain an id from a tiny `Editor::new_from_text` fixture instead; do not change BufferId's visibility.)

- [ ] **Step 3: `close_buffer` + `close_buffer_now`** (workspace.rs :99-123 replaced; the doc comment updated to describe the prompt). Complete code:

```rust
/// Close the active buffer. Scratch → no-op (status set). Dirty → raise the
/// Save/Discard/Cancel close-confirm prompt (C4) — unless another save/quit
/// flow has pending state, in which case refuse with a status (the shared
/// Cancel/Esc arms would clobber that flow's pendings — spec D1 busy guard).
/// Clean → close immediately. Last ordinary buffer → replaced with a fresh
/// empty untitled. New active = same-index neighbor.
pub fn close_buffer(editor: &mut Editor) {
    let id = editor.active().id;
    if editor.is_scratch(id) { editor.status = "can't close the scratch buffer".into(); return; }
    if editor.is_dirty(id) {
        if editor.pending_after_save.is_some() || editor.pending_save_as.is_some() || editor.quit_drain.is_some() {
            editor.status = "another save or quit is in progress — try again".into();
            return;
        }
        let name = buffer_display_name(editor, id);
        editor.open_prompt(crate::prompt::Prompt::close_confirm(&name, id));
        return;
    }
    close_buffer_now(editor, id);
}

/// Close `id` unconditionally (no dirty check) — the shared mechanics behind
/// the clean-path close, the Discard arm, and the post-save close (spec D2).
/// Per-case BY DESIGN: when `id` is not active, the viewer must not be yanked
/// (no switch_to), and the last-ordinary replacement targets id's OWN slot —
/// never buffers[active], which would overwrite the scratch and dangle
/// scratch_id.
pub(crate) fn close_buffer_now(editor: &mut Editor, id: BufferId) {
    let Some(i) = editor.buffers.iter().position(|b| b.id == id) else {
        editor.status = "buffer already closed".into();
        return;
    };
    let ordinary = editor.buffers.iter().filter(|b| !editor.is_scratch(b.id)).count();
    if ordinary <= 1 {
        // Last ordinary buffer: replace id's own slot with a fresh empty untitled.
        let nid = editor.alloc_id();
        let area = editor.buffers[i].view.area;
        let was_active = i == editor.active;
        editor.buffers[i] = crate::editor::Buffer::from_text(nid, "\n", None, area);
        editor.mru.retain(|&x| x != id);
        if was_active {
            editor.touch_mru(nid);
            crate::derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
        } else {
            // Untouched fresh buffer: back of the MRU, not most-recent (spec D2 —
            // fronting it would break the weak MRU-front == active convention).
            editor.mru.push(nid);
        }
        editor.status = String::new();
        return;
    }
    if i == editor.active {
        editor.mru.retain(|&x| x != id);
        editor.buffers.remove(i);
        let new_idx = i.min(editor.buffers.len() - 1);
        switch_to(editor, new_idx);
    } else {
        // The viewer stays put: remove id's slot, then re-point `active` by the
        // previously-active buffer's ID (its index shifts down when i < active).
        let active_id = editor.active().id;
        editor.mru.retain(|&x| x != id);
        editor.buffers.remove(i);
        if let Some(na) = editor.buffers.iter().position(|b| b.id == active_id) {
            editor.active = na;
        }
    }
    editor.status = String::new();
}
```

- [ ] **Step 4: the resolve arms** (prompts.rs, immediately after `ReviewDiscard`'s arm :131-136, mirroring the `ReviewSave`/`ReviewDiscard` clear-act-return shape):

```rust
        PromptAction::CloseSave { id } => {
            editor.prompt = None;
            let mut ctx = Ctx { editor, clock, executor: ex, msg_tx: msg_tx.clone() };
            crate::save::dispatch_save_then(&mut ctx, crate::editor::PostSaveAction::CloseBuffer { id });
            return;
        }
        PromptAction::CloseDiscard { id } => {
            editor.prompt = None;
            crate::workspace::close_buffer_now(editor, id);
            return;
        }
```

- [ ] **Step 5: the apply arm** (jobs_apply.rs, third arm of the `match action` at :33, after `ContinueQuitDrain`'s):

```rust
                crate::editor::PostSaveAction::CloseBuffer { id } => {
                    editor.pending_after_save = None;
                    if saved_this && !editor.is_dirty(id) {
                        // is_dirty on the ACTION's id, not the result's buffer_id —
                        // only the Save-As divergence separates them, and the
                        // buffer_id misreading would close a still-dirty buffer
                        // (spec D3). close_buffer_now re-reads counts at apply time.
                        crate::workspace::close_buffer_now(editor, id);
                        editor.status = "saved — closed".into();
                    } else if saved_this {
                        // Edited during the in-flight save: do NOT close.
                        editor.status = "edited during save — close cancelled".into();
                    }
                    // !saved_this: no close; the merge's own status stands (error
                    // text, or empty for a vanished target) — mirrors
                    // ContinueQuitDrain's abort, NOT Quit's leave-armed (an armed
                    // close would fire on the user's next manual save). (The
                    // saved-branch's "saved — closed" harmlessly shadows the
                    // vanished-id status — unreachable for the pending's own id.)
                }
```

- [ ] **Step 6: run everything to GREEN.** `cargo test -p wordcartel close_` then the full suite. The Step-1 pins now pass; every pre-existing workspace clean-path test (:261-314) must pass UNCHANGED (close_buffer's clean path delegates to close_buffer_now with `i == active`, reproducing today's behavior exactly).

- [ ] **Step 7: the state-machine test battery.** Add, using the grounding idioms (quit_tmp/SEQ for temp files, `InlineExecutor::default()` + `ex.drain()` + `apply_job_outcome` for saves, the version/saved_version dirty idiom, `resolve_prompt(...)` direct calls):
  - workspace.rs: `close_buffer_now_by_id_closes_inactive_buffer` (three buffers incl. scratch; close a NON-active id → viewed buffer still active by ID, count drops); `close_buffer_now_nonactive_normal_keeps_view` (arrange the removed index BELOW the active one → `editor.active` re-pointed, `active().id` unchanged); `close_buffer_now_vanished_id_is_noop_with_status`.
  - prompts.rs: `close_save_arms_pending_after_save_with_close_variant` (dirty named buffer, `resolve_prompt(CloseSave { id })` → `pending_after_save` is `Some` with `action == CloseBuffer { id }`, matching buffer_id/version); `close_discard_closes_immediately_and_leaves_swap` (write a real swap via `crate::swap::swap_path(Some(&p))` + `crate::swap::write_atomic(&sp, "stub")`, resolve `CloseDiscard { id }` → buffer gone AND `sp.exists()` — decision 1's pin); `close_save_on_unnamed_buffer_opens_save_as_with_carry` (unnamed dirty → `pending_save_as == Some(CloseBuffer { .. })`, minibuffer open).
  - jobs_apply.rs (the drain-loop template from `quit_save_all_drains_named_dirty_then_quits`
    — NOTE the template, `quit_tmp`, and `SEQ` live in APP.RS's test module (~:2117-:2250,
    SEQ ~:1658) and are module-private: transcribe `quit_tmp`/`SEQ` locally into
    jobs_apply.rs's test module; Fable plan r3):
    `close_after_save_closes_on_matching_result` (CloseSave on a dirty named buffer → drain → buffer count drops, the correct NEIGHBOR is active by ID, file on disk updated, status `"saved — closed"`);
    `close_cancelled_when_edited_during_flight` (arm via `resolve_prompt(CloseSave{id})` with a REAL executor deferral — or arm `pending_after_save` manually as the quit sibling `quit_after_save_cancelled_when_edited_during_flight` does — then dirty the buffer again before applying the result → buffer stays, status verbatim);
    `close_not_performed_on_save_failure` (the symlink-target trick verbatim from `quit_drain_aborts_on_save_failure`, `#[cfg(unix)]` guarded → buffer stays, `pending_after_save` is None, error status contains "symlink");
    `close_result_for_wrong_buffer_is_stale_noop` (arm for buffer A, deliver a matching-versioned result for buffer B → nothing closes; keys off the `fire` predicate);
    `close_after_save_last_ordinary_while_scratch_active` (**the Fable C1 corruption pin**: `[X, scratch]`, CloseSave{X}, `goto_scratch` during the flight, apply the result → scratch INTACT — `scratch_id` still valid, scratch content untouched — a fresh untitled sits in X's slot, AND the fresh id is at the BACK of the MRU with scratch still at the front — the D2 MRU decision's pin, Fable plan r3);
    `close_after_save_last_ordinary_recheck` (**the spec's flight-time recheck, distinct
    from the scratch pin — Codex plan r1**: THREE buffers `[X, Y, scratch]`, CloseSave{X},
    then `close_buffer_now(&mut e, y_id)` during the flight (Y clean), apply X's result →
    the ordinary count re-read at APPLY time is 1, so the last-ordinary path fires:
    a fresh untitled replaces X's slot rather than a bare remove);
    `close_save_on_conflicted_file_raises_external_mod_and_does_not_arm` (arrange the external-mod state the way `dispatch_save`'s conflict tests do — see save.rs's external-mod test family for the fingerprint idiom — → `prompt.is_some()`, `pending_after_save.is_none()`).
  Run each family as separate single-filter invocations; then the full gates.

- [ ] **Step 8: commit** — `feat(c4): close-buffer Save/Discard/Cancel prompt — id-carrying state machine, per-case close_buffer_now, busy guard`.

---

### Task 2: the timeout branch + the quit-supersedes clear

**Files:**
- Modify: `wordcartel/src/app.rs` (timeout arm :1423-1445 + tests), `wordcartel/src/commands.rs` (the Quit dispatch :526-538)

**Interfaces:**
- Consumes: Task 1's `PostSaveAction::CloseBuffer`.
- Produces: complete behavioral coverage; nothing new for Task 3.

- [ ] **Step 1: failing tests first** (app.rs test module; the timeout test drives the
  extracted `save_timeout_tick` helper from Step 2 directly — NO sibling timeout test
  exists anywhere and the run()-local block is unreachable from `reduce`; Fable plan r3
  confirmed by grep):

```rust
    #[test]
    fn close_save_timeout_cancels_with_status() {
        // Drives the EXTRACTED helper directly (Codex plan r1: the timeout lives in
        // run(), unreachable via reduce). Arrange a CloseBuffer pending at t=0; call
        // save_timeout_tick(&mut e, SAVE_QUIT_TIMEOUT_MS + 1) → pending cleared,
        // status "save timed out — close cancelled", buffer open, NO prompt.
        // Also pin the extraction is faithful: a Quit-variant pending re-raises
        // quit_confirm through the same helper.
        …arrange a dirty named buffer; arm pending_after_save manually with
        action CloseBuffer{id}, at_ms: 0; call the helper; assert the four
        conditions; then re-arm with Quit and assert the re-prompt…
    }

    #[test]
    fn quit_dispatch_cancels_pending_close() {
        // Arm pending_after_save = CloseBuffer{X} (manually), dispatch the quit
        // command (commands::run Quit via the registry or reduce ctrl-q idiom) →
        // pending_after_save is None BEFORE the quit prompt raises; cancel the
        // quit → still None; a later matching save result closes NOTHING.
        // Repeat with pending_save_as = Some(CloseBuffer{X}).
        …
    }

    #[test]
    fn esc_on_close_prompt_cancels_cleanly() {
        // Raise the close prompt via close_buffer on a dirty buffer; send Esc
        // through reduce → prompt None, buffer open, pending_* all None.
        …
    }

    #[test]
    fn close_dirty_scratch_still_refuses_via_scratch_guard() {
        // Make the SCRATCH buffer the active one, dirty it, close_buffer →
        // scratch-guard status, NO prompt (the guard order pin).
        …
    }
```

(The `…` bodies follow the named sibling idioms exactly — the implementer locates each
cited fixture and mirrors it; every assertion listed is required. The timeout test
drives the EXTRACTED `save_timeout_tick` helper from Step 2 directly — the block lives
in `run()` and is unreachable through `reduce` (Codex plan r1/r2); do NOT attempt a
`reduce(Msg::Tick)` route. RED: the first two fail — the CloseBuffer variant has no
timeout branch yet, and quit leaves the pending armed.)

- [ ] **Step 2: the timeout branch — via an extracted seam (Codex plan r1: the timeout
  block lives in `run()`, app.rs:1419-1444, NOT in reduce's `Msg::Tick` arm — no test can
  reach it through `reduce`).** Extract the existing block into a module-level helper,
  behavior-preserving:

```rust
/// Save-timeout disposition (extracted from run()'s tick so it is testable — C4).
/// Returns without effect while no pending save is overdue.
pub(crate) fn save_timeout_tick(editor: &mut Editor, now: u64) {
    if let Some(p) = &editor.pending_after_save {
        let waited = now.saturating_sub(p.at_ms);
        if waited > SAVE_QUIT_TIMEOUT_MS {
            // Compiler-exhaustive on purpose (Codex plan r2): a future
            // PostSaveAction variant must NOT compile silently past this helper.
            let action = p.action.clone();
            editor.pending_after_save = None;
            match action {
                crate::editor::PostSaveAction::Quit => {
                    editor.open_prompt(crate::prompt::Prompt::quit_confirm());
                    editor.status = "Save still running — choose again".into();
                }
                crate::editor::PostSaveAction::ContinueQuitDrain => {
                    editor.quit_drain = None;
                    editor.quit_drain_advance = false;
                    editor.status = "save timed out — quit cancelled".into();
                }
                crate::editor::PostSaveAction::CloseBuffer { .. } => {
                    // C4: a close is not a session-ending action the user is
                    // waiting on — cancel without re-prompting (spec D3).
                    editor.status = "save timed out — close cancelled".into();
                }
            }
        }
    }
}
```

  The `run()` site (app.rs:1423-1445) becomes `save_timeout_tick(&mut editor, now);` —
  move the existing comments INTO the helper (they document the Quit/drain branches);
  `SAVE_QUIT_TIMEOUT_MS` moves from run()-local (app.rs:1376) to module scope (same
  value, `const SAVE_QUIT_TIMEOUT_MS: u64 = 5_000;` above the helper) — the `sq_deadline`
  reference at app.rs:1448 keeps working. The pre-existing final `else`
  ("Save still running — try again") was ALREADY dead code (both existing variants have
  branches — Codex r2 confirmed) and is dropped; the helper uses a compiler-exhaustive
  `match` (not the flag trio) so a future `PostSaveAction` variant cannot compile
  silently past it. Record both facts in the report.

- [ ] **Step 3: the quit-supersedes clear.** At the TOP of `Command::Quit`'s arm (commands.rs:526, before `any_dirty`):

```rust
            // C4/I1 (user-ratified): quit supersedes — and cancels — a pending
            // close. Clear CloseBuffer-carrying pendings so a cancelled quit
            // leaves no ghost close armed to fire on the next manual save.
            // Foreign quit/drain pendings are the existing flow's business.
            if editor.pending_after_save.as_ref()
                .is_some_and(|p| matches!(&p.action, crate::editor::PostSaveAction::CloseBuffer { .. })) {
                editor.pending_after_save = None;
            }
            if matches!(&editor.pending_save_as, Some(crate::editor::PostSaveAction::CloseBuffer { .. })) {
                editor.pending_save_as = None;
            }
```

- [ ] **Step 4: GREEN + full gates.**

- [ ] **Step 5: commit** — `feat(c4): close timeout disposition + quit supersedes-and-cancels a pending close`.

---

### Task 3: e2e journeys + smoke

**Files:**
- Modify: `wordcartel/src/e2e.rs`

**Interfaces:** consumes Tasks 1-2; produces the shipped user-visible pins.

- [ ] **Step 1: the journeys** (Harness idioms per the grounding — `h.ctrl('p')`, `h.type_str(..)`, `h.key(KeyCode::Enter)`, `h.key(KeyCode::Char('s'))`, `h.screen_contains(..)`; saves run inline under the Harness's InlineExecutor):
  - `journey_close_dirty_save_and_close`: dirty NAMED buffer (Harness with a quit_tmp-style real path; type to dirty it) + a second buffer so close has a neighbor → palette-dispatch `close_buffer` (ctrl-p, type "close", Enter) → `screen_contains` the close-confirm message → `Char('s')` → buffer closed (count drops, neighbor visible), file on disk contains the typed text, status "saved — closed".
  - `journey_close_dirty_discard_leaves_file_and_swap`: same arrange plus a real swap file written via the swap API → `Char('d')` → buffer closed, file on disk UNCHANGED, `sp.exists()` still true.
  - (The `c`/Esc paths are unit-pinned in Task 2 — no journey needed.)

- [ ] **Step 2: full gates + smoke.** Run `scripts/smoke/run.sh` and QUOTE the one-line summary verbatim in the report (advisory).

- [ ] **Step 3: commit** — `feat(c4): close-buffer e2e journeys`.

---

## Verification appendix (final whole-branch review charge)

- The three user decisions hold: a real swap survives Discard (the T1 pin); NO keymap.rs changes anywhere in the branch; the quit-clear fires only on `CloseBuffer`-carrying pendings.
- The sanctioned pin reversal is the ONLY meaning-change to an existing test; the workspace clean-path family (:261-314) passes unmodified.
- The Fable C1 corruption pin (`close_after_save_last_ordinary_while_scratch_active`) passes, and `close_buffer_now`'s non-active arms are exercised by tests, not just written.
- `apply_panic` remains action-agnostic and UNTOUCHED (spec D3's verify-don't-modify —
  it keys only on buffer_id/version, jobs_apply.rs:99-105, so the new variant is covered
  with zero changes).
- `PromptAction` still derives `Copy`; no new `#[allow]`; no `unsafe`; registry.rs untouched.
- Pre-merge: smoke verbatim + a live tmux sanity (dirty buffer, palette-dispatch Close Buffer, exercise s/d/c across three runs).
- Controller merge-time bookkeeping: backlog C4 → SHIPPED (no binding — deferred to A5; swap-survives-discard convention), working order advances to C2, memory tick.
