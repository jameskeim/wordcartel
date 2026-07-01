# Incremental-soundness eventual-consistency reconcile — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop `derive::rebuild`'s per-draw full parse (large-doc responsiveness) and add a debounced background reconcile that converges the active buffer's block tree to `full_parse` at rest — the convergence theorem.

**Architecture:** Shell-only (no core changes). `rebuild` becomes version-memoized (reparse only when the text changed; non-edit draws refresh layout/folds from the existing `document.blocks`). A per-buffer `ReconcileStore` (mirrors `DiagStore`) + a `JobKind::Reparse` Executor job that full-parses on a worker and version-check-diff-merges the result.

**Tech Stack:** Rust, ropey, the existing `jobs::Executor` substrate, `wordcartel_core::block_tree`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-01-wordcartel-incremental-soundness-reconcile-design.md` (Codex-clean).
- Gates: `cargo test -p wordcartel -p wordcartel-core` green; `cargo build`/`test --no-run` warning-free; **`cargo clippy --workspace --all-targets` clean (the deny gate is now LIVE — new code must not introduce warnings)**; NO `cargo fmt`; house style (em-dash `—`).
- No `wordcartel-core` changes: `incremental_update_instrumented_src` (block_tree.rs:541), `full_parse_rope` (block_tree.rs:516), `empty_tree`, `BlockTree: PartialEq/Eq`, `WidenReason`/`UpdateOutcome` (block_tree.rs:472) are all already public.
- Convergence theorem: no edits for `RECONCILE_DEBOUNCE_MS` ⇒ `document.blocks == full_parse(text)`.
- Trailers on every commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

## File Structure

- Create `wordcartel/src/reconcile.rs` — `ReconcileStore`, `RECONCILE_DEBOUNCE_MS`, `reconcile_due`, `dispatch_reconcile` (mirrors `diagnostics_run.rs`).
- Modify `wordcartel/src/editor.rs` — add `Buffer.reconcile: reconcile::ReconcileStore`.
- Modify `wordcartel/src/lib.rs` — `mod reconcile;`.
- Modify `wordcartel/src/derive.rs` — version-memoized two-phase `rebuild` + `rebuild_downstream`.
- Modify `wordcartel/src/jobs.rs` — `JobKind::Reparse` + `is_stale` arm; widen the `merge` doc comment.
- Modify `wordcartel/src/app.rs` — `apply_panic` `Reparse` arm; main-loop arm + deadline + `Msg::Tick` dispatch.

---

### Task 1: `reconcile` module — `ReconcileStore` + `Buffer.reconcile` field

**Files:**
- Create: `wordcartel/src/reconcile.rs`
- Modify: `wordcartel/src/lib.rs` (add `mod reconcile;`), `wordcartel/src/editor.rs` (add the field + init)

**Interfaces:**
- Produces: `reconcile::ReconcileStore { blocks_version: u64, maybe_stale: bool, due_at: Option<u64>, in_flight_version: Option<u64>, armed_for_version: u64 }` (derives `Debug, Default, Clone`); `reconcile::RECONCILE_DEBOUNCE_MS: u64`; `reconcile::reconcile_due(&ReconcileStore, now: u64) -> bool`.

- [ ] **Step 1: Create `reconcile.rs` with the store + `reconcile_due` + a failing test**

```rust
//! Block-tree reconcile runtime (shell): per-buffer store + debounce helper +
//! the background reparse job dispatch. Mirrors `diagnostics_run.rs`. Gives the
//! convergence theorem: quiescence ⇒ `document.blocks == full_parse(text)`.

use crate::editor::Editor;
use crate::jobs::{Executor, Job, JobKind, JobResult, ResultClass};

/// Debounce before a settled buffer's tree is reconciled to `full_parse`.
/// ~150 ms — long enough not to fire mid-burst, short enough to feel instant.
pub const RECONCILE_DEBOUNCE_MS: u64 = 150;

/// Per-buffer reconcile state. `blocks_version` is the memoization key for
/// `derive::rebuild` (the document version `document.blocks` was built for).
#[derive(Debug, Default, Clone)]
pub struct ReconcileStore {
    /// The document version `document.blocks` currently reflects.
    pub blocks_version: u64,
    /// The current tree may differ from `full_parse` (set on an incremental
    /// `Local`/`WidenToEnd` update; cleared whenever a full parse establishes it).
    pub maybe_stale: bool,
    /// Debounce deadline: dispatch a reconcile once `now >= due_at`.
    pub due_at: Option<u64>,
    /// A reconcile job is running for this version (blocks re-dispatch).
    pub in_flight_version: Option<u64>,
    /// The document version the debounce was last armed for (so idle Ticks do
    /// not re-arm and push the deadline forever).
    pub armed_for_version: u64,
}

/// A reconcile is due if the tree may be stale, nothing is in flight, and the
/// debounce deadline has been reached.
pub fn reconcile_due(store: &ReconcileStore, now: u64) -> bool {
    store.maybe_stale
        && store.in_flight_version.is_none()
        && matches!(store.due_at, Some(t) if now >= t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_due_requires_stale_armed_and_not_in_flight() {
        let mut s = ReconcileStore { maybe_stale: true, due_at: Some(100), ..Default::default() };
        assert!(!reconcile_due(&s, 99), "not yet due");
        assert!(reconcile_due(&s, 100), "due at deadline");
        s.in_flight_version = Some(1);
        assert!(!reconcile_due(&s, 200), "in-flight blocks dispatch");
        s.in_flight_version = None;
        s.maybe_stale = false;
        assert!(!reconcile_due(&s, 200), "not stale → nothing to do");
        s.maybe_stale = true;
        s.due_at = None;
        assert!(!reconcile_due(&s, 200), "not armed");
    }
}
```

- [ ] **Step 2: Wire the module** — in `wordcartel/src/lib.rs`, add `mod reconcile;` (grouped with the other `mod` declarations, matching house order).

- [ ] **Step 3: Add the `Buffer` field** — in `editor.rs`, add to the `Buffer` struct (after the `diagnostics` field at editor.rs:114, matching style):

```rust
    // 5f: per-buffer diagnostics store
    pub diagnostics: crate::diagnostics_run::DiagStore,
    /// per-buffer block-tree reconcile store (incremental-soundness effort)
    pub reconcile: crate::reconcile::ReconcileStore,
```

Initialize it in every `Buffer { … }` construction site (compile errors point to each; `Buffer::from_text` is the main one). `ReconcileStore::default()` is correct as the initial state: `Buffer::from_text` full-parses at construction for version 0, so `blocks_version: 0, maybe_stale: false` matches. Use `reconcile: crate::reconcile::ReconcileStore::default(),`.

- [ ] **Step 4: Run + gates + commit**

`cargo test -p wordcartel --lib reconcile` green; `cargo build -p wordcartel` + clippy clean.
```bash
git add wordcartel/src/reconcile.rs wordcartel/src/lib.rs wordcartel/src/editor.rs
git commit -m "feat(reconcile): ReconcileStore + Buffer.reconcile field + reconcile_due"   # + trailers
```

---

### Task 2: version-memoized two-phase `derive::rebuild`

**Files:**
- Modify: `wordcartel/src/derive.rs` (`rebuild` + extract `rebuild_downstream`)

**Interfaces:**
- Consumes: `reconcile::ReconcileStore` (Task 1); `block_tree::{incremental_update_instrumented_src, full_parse_rope, empty_tree, WidenReason, UpdateOutcome}`.
- Produces: `pub(crate) fn rebuild_downstream(editor: &mut Editor)` (the fold/outline/layout phase, callable without re-parsing).

**Real anchors:** `rebuild` derive.rs:82; parse phase 86–109; downstream (fold reconcile 115–124, layout loop 131–178); `apply_parse_result` derive.rs:189. `Buffer::apply` bumps `version` + sets `pre_edit_rope`/`last_edit` (editor.rs:189–191); `undo`/`redo` bump `version` + CLEAR them (editor.rs:226–228/245–247).

- [ ] **Step 1: Write the failing tests** (in `derive.rs`'s `#[cfg(test)] mod tests`; build the editor with `Editor::new_from_text`; use `commands::build_multi_replace` + `Buffer::apply` to make edits, as other derive tests do)

```rust
#[test]
fn rebuild_skips_reparse_when_version_unchanged() {
    use wordcartel_core::block_tree;
    let mut e = crate::editor::Editor::new_from_text("# H\n\nbody\n", None, (80, 24));
    // After construction, blocks_version tracks version 0.
    e.active_mut().reconcile.blocks_version = e.active().document.version;
    // Plant a sentinel tree that differs from full_parse, with NO pending edit.
    let sentinel = block_tree::empty_tree(e.active().document.buffer.len());
    e.active_mut().document.blocks = sentinel.clone();
    e.active_mut().pre_edit_rope = None;
    e.active_mut().last_edit = None;
    crate::derive::rebuild(&mut e);
    // version == blocks_version → parse phase skipped → sentinel survives.
    assert_eq!(e.active().document.blocks, sentinel, "non-edit rebuild must not reparse");
}

#[test]
fn rebuild_reparses_and_sets_stale_on_incremental_edit() {
    use wordcartel_core::block_tree;
    let mut e = crate::editor::Editor::new_from_text("hello\n", None, (80, 24));
    e.active_mut().reconcile.blocks_version = e.active().document.version;
    e.active_mut().reconcile.maybe_stale = false;
    // an ordinary insert (routes through Buffer::apply → sets pre_edit_rope/last_edit, bumps version)
    let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "X".into())], 0);
    let txn = wordcartel_core::history::Transaction::new(cs);
    struct C; impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { 0 } }
    e.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C);
    crate::derive::rebuild(&mut e);
    assert_eq!(e.active().reconcile.blocks_version, e.active().document.version);
    // a plain in-paragraph insert is Local → maybe_stale set
    assert!(e.active().reconcile.maybe_stale, "incremental Local/WidenToEnd → maybe_stale");
    assert_eq!(e.active().document.blocks, block_tree::full_parse(&e.active().document.buffer.to_string()));
}

#[test]
fn rebuild_full_parses_and_clears_stale_on_undo() {
    let mut e = crate::editor::Editor::new_from_text("abc\n", None, (80, 24));
    let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "Z".into())], 0);
    let txn = wordcartel_core::history::Transaction::new(cs);
    struct C; impl wordcartel_core::history::Clock for C { fn now_ms(&self) -> u64 { 0 } }
    e.active_mut().apply(txn, edit, wordcartel_core::history::EditKind::Other, &C);
    crate::derive::rebuild(&mut e);
    e.active_mut().undo(); // bumps version, clears pre_edit_rope/last_edit
    e.active_mut().reconcile.maybe_stale = true; // pretend stale before undo's rebuild
    crate::derive::rebuild(&mut e);
    assert!(!e.active().reconcile.maybe_stale, "undo → full parse → maybe_stale cleared");
    assert_eq!(e.active().reconcile.blocks_version, e.active().document.version);
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test -p wordcartel --lib derive::tests::rebuild_` → FAIL (rebuild not yet gated; no `reconcile` reads).

- [ ] **Step 3: Refactor `rebuild`** — replace the parse section (derive.rs:82–109) with a version-gated parse phase, and move the downstream (115–178) into `rebuild_downstream`. New `rebuild`:

```rust
pub fn rebuild(editor: &mut Editor) {
    let version = editor.active().document.version;
    let blocks_version = editor.active().reconcile.blocks_version;

    // Parse phase: only when the text actually changed since the tree was built.
    if version != blocks_version {
        let new_rope = editor.active().document.buffer.snapshot(); // O(1) ropey clone
        let new_len = new_rope.len_bytes();
        let maybe_old_rope = editor.active_mut().pre_edit_rope.take();
        let maybe_edit = editor.active_mut().last_edit.take();

        // Incremental ONLY when the tree is exactly one version behind AND the
        // pending edit bridges that gap (a single edit since the last parse).
        // Any gap (undo/redo clear the edit info; multi-edit-before-rebuild) →
        // a safe full parse.
        let one_behind = version == blocks_version.wrapping_add(1);
        let (new_blocks, stale) = if one_behind {
            if let (Some(old_rope), Some(edit)) = (&maybe_old_rope, &maybe_edit) {
                match crate::panicx::catch(|| {
                    block_tree::incremental_update_instrumented_src(
                        &editor.active().document.blocks, old_rope, edit, &new_rope,
                    )
                }) {
                    Ok(outcome) => {
                        let stale = matches!(
                            outcome.reason,
                            block_tree::WidenReason::Local | block_tree::WidenReason::WidenToEnd
                        );
                        (outcome.tree, stale)
                    }
                    // A parse panic → empty-tree fallback (NOT full_parse) → stale.
                    Err(_) => (parse_degraded_empty(editor, new_len), true),
                }
            } else {
                full_parse_phase(editor, &new_rope, new_len)
            }
        } else {
            full_parse_phase(editor, &new_rope, new_len)
        };

        editor.active_mut().document.blocks = new_blocks;
        editor.active_mut().reconcile.blocks_version = version;
        editor.active_mut().reconcile.maybe_stale = stale;
    }

    rebuild_downstream(editor);
}

/// Full parse for the current rope; returns (tree, stale=false). A panic falls
/// back to the empty tree (stale=true). Uses the M4-rest degraded-status helper.
fn full_parse_phase(editor: &mut Editor, new_rope: &ropey::Rope, new_len: usize) -> (block_tree::BlockTree, bool) {
    match crate::panicx::catch(|| block_tree::full_parse_rope(new_rope)) {
        Ok(tree) => {
            if editor.parse_degraded { editor.parse_degraded = false; editor.status.clear(); }
            (tree, false)
        }
        Err(_) => (parse_degraded_empty(editor, new_len), true),
    }
}

/// Set the degraded status (dedup) and return the empty-tree fallback.
fn parse_degraded_empty(editor: &mut Editor, new_len: usize) -> block_tree::BlockTree {
    if !editor.parse_degraded {
        editor.parse_degraded = true;
        editor.status = "markdown parse failed — styling may be stale".to_string();
    }
    block_tree::empty_tree(new_len)
}
```

(`apply_parse_result` is now subsumed by `full_parse_phase`/`parse_degraded_empty` — remove it and update its callers, or keep it if other callers exist; the plan-confirm found `rebuild` was its only caller, so remove it.)

- [ ] **Step 4: Extract `rebuild_downstream`** — move the fold-reconcile + layout code (the old derive.rs:111–178, everything after the parse) verbatim into:

```rust
/// The downstream-of-tree phase: reconcile fold anchors + build the FoldView +
/// refresh the visible-line layout cache from the CURRENT `document.blocks`.
/// Runs every draw and does NOT reparse; also called by the reconcile merge.
pub(crate) fn rebuild_downstream(editor: &mut Editor) {
    // ... the exact body of the old derive.rs:111–178 (fold reconcile block,
    // FoldView::compute, the visible-range snapshot, and the layout loop) ...
}
```

- [ ] **Step 5: Run tests** — `cargo test -p wordcartel --lib derive` green (the 3 new + all existing derive tests). Full `cargo test -p wordcartel` green (existing render/nav tests still pass — the downstream phase is byte-identical to before for a given tree).

- [ ] **Step 6: Gates + commit** — clippy clean; no fmt.
```bash
git add wordcartel/src/derive.rs
git commit -m "feat(derive): version-memoized two-phase rebuild (perf) + maybe_stale flag"   # + trailers
```

---

### Task 3: `JobKind::Reparse` + dispatch + version-checked diff-merge

**Files:**
- Modify: `wordcartel/src/jobs.rs` (`JobKind::Reparse` variant + `is_stale` arm + `merge` doc widen)
- Modify: `wordcartel/src/app.rs` (`apply_panic` `Reparse` arm)
- Modify: `wordcartel/src/reconcile.rs` (`dispatch_reconcile`)

**Interfaces:**
- Consumes: `jobs::{Job, JobResult, JobKind, JobOutcome, ResultClass, Executor}`; `block_tree::full_parse_rope`; `BlockTree: PartialEq`.
- Produces: `reconcile::dispatch_reconcile(editor: &mut Editor, ex: &dyn jobs::Executor)`.

- [ ] **Step 1: Add the `JobKind` variant + `is_stale` arm** — `jobs.rs`:

```rust
pub enum JobKind {
    Save,      // one-shot, user-initiated: always applies
    SwapWrite, // one-shot housekeeping: always applies (status only)
    Reparse,   // coalescible background block-tree reconcile; version-checked in merge
    #[cfg(test)]
    CoalesceProbe,
}
```
In `is_stale` (jobs.rs:66), add `Reparse` to the "never stale here" arm — the version check + in-flight clear happen INSIDE the merge (so the merge ALWAYS runs for an open buffer and can clear `in_flight_version`, avoiding a stuck in-flight):
```rust
                match r.kind {
                    JobKind::Save | JobKind::SwapWrite | JobKind::Reparse => false,
                    #[cfg(test)]
                    JobKind::CoalesceProbe => r.version != b.document.version,
                }
```
Widen the `JobResult.merge` doc comment (jobs.rs:44-46) to: `By contract this touches only non-document bookkeeping and DERIVED document caches (e.g. document.blocks, regenerable from text); any document-TEXT change must route through editor.apply.`

- [ ] **Step 2: Add the `apply_panic` `Reparse` arm** — `app.rs:200` match:
```rust
        JobKind::Reparse => {
            // A panicked reparse (e.g. the pulldown residual): drop the round,
            // leave document.blocks unchanged; clear in-flight so a later
            // reconcile can re-dispatch. No status noise.
            if let Some(b) = editor.by_id_mut(buffer_id) { b.reconcile.in_flight_version = None; }
        }
```

- [ ] **Step 3: Write the failing convergence + merge test** (in `reconcile.rs` tests; use `jobs::InlineExecutor` — it runs the job synchronously so `dispatch → drain → apply` is deterministic):

```rust
#[test]
fn reconcile_converges_a_diverged_tree_to_full_parse() {
    use wordcartel_core::block_tree;
    let mut e = crate::editor::Editor::new_from_text("para\n", None, (80, 24));
    let bid = e.active().id;
    let v = e.active().document.version;
    // Plant a deliberately-wrong tree at the current version (simulating a
    // diverged incremental result), flagged stale.
    e.active_mut().document.blocks = block_tree::empty_tree(e.active().document.buffer.len());
    e.active_mut().reconcile.blocks_version = v;
    e.active_mut().reconcile.maybe_stale = true;
    let correct = block_tree::full_parse(&e.active().document.buffer.to_string());
    assert_ne!(e.active().document.blocks, correct, "precondition: tree is diverged");

    let ex = crate::jobs::InlineExecutor::new();
    dispatch_reconcile(&mut e, &ex);
    for o in ex.drain() { crate::app::apply_outcome(o, &mut e); }

    assert_eq!(e.active().document.blocks, correct, "reconcile converges to full_parse");
    assert!(!e.active().reconcile.maybe_stale, "stale cleared");
    assert_eq!(e.active().reconcile.blocks_version, v);
    let _ = bid;
}

#[test]
fn reconcile_discards_when_version_advanced() {
    use wordcartel_core::block_tree;
    let mut e = crate::editor::Editor::new_from_text("para\n", None, (80, 24));
    e.active_mut().reconcile.maybe_stale = true;
    e.active_mut().reconcile.blocks_version = e.active().document.version;
    let planted = block_tree::empty_tree(e.active().document.buffer.len());
    e.active_mut().document.blocks = planted.clone();

    let ex = crate::jobs::InlineExecutor::new();
    dispatch_reconcile(&mut e, &ex); // snapshots the current version
    // an edit lands before the (synchronous, here) merge is applied:
    e.active_mut().document.version += 1;
    for o in ex.drain() { crate::app::apply_outcome(o, &mut e); }

    assert_eq!(e.active().document.blocks, planted, "stale reconcile did not clobber the newer state");
    assert!(e.active().reconcile.in_flight_version.is_none(), "in-flight cleared even on discard");
}
```

- [ ] **Step 4: Implement `dispatch_reconcile`** — `reconcile.rs`:

```rust
/// Snapshot the active buffer + dispatch a background full-parse reconcile.
/// Sets `in_flight_version` and clears the debounce deadline (consumed).
pub fn dispatch_reconcile(editor: &mut Editor, ex: &dyn Executor) {
    let b = editor.active();
    let buffer_id = b.id;
    let version = b.document.version;
    let rope = b.document.buffer.snapshot(); // O(1) ropey clone, moved to the worker
    editor.active_mut().reconcile.in_flight_version = Some(version);
    editor.active_mut().reconcile.due_at = None;

    let job = Job {
        buffer_id,
        class: ResultClass::BufferLocal,
        version,
        kind: JobKind::Reparse,
        run: Box::new(move || {
            let tree = wordcartel_core::block_tree::full_parse_rope(&rope);
            JobResult {
                buffer_id,
                class: ResultClass::BufferLocal,
                version,
                kind: JobKind::Reparse,
                merge: Box::new(move |editor: &mut Editor| {
                    if let Some(b) = editor.by_id_mut(buffer_id) {
                        // Version-check INSIDE the merge (the version-discard): only
                        // adopt the tree if the buffer is still at the job's version.
                        if b.document.version == version {
                            if b.document.blocks != tree {
                                b.document.blocks = tree;
                            }
                            b.reconcile.blocks_version = version;
                            b.reconcile.maybe_stale = false;
                            // The pre-draw derive::rebuild will refresh downstream
                            // (version == blocks_version → skip parse → downstream).
                        }
                        // Clear in-flight regardless (the reconcile completed), so a
                        // later reconcile can dispatch.
                        b.reconcile.in_flight_version = None;
                    }
                }),
            }
        }),
    };
    ex.dispatch(job);
}
```

- [ ] **Step 5: Run tests** — `cargo test -p wordcartel --lib reconcile` green (both new tests + Task 1's). Full `cargo test -p wordcartel` green.

- [ ] **Step 6: Gates + commit** — clippy clean; no fmt.
```bash
git add wordcartel/src/jobs.rs wordcartel/src/app.rs wordcartel/src/reconcile.rs
git commit -m "feat(reconcile): JobKind::Reparse + dispatch_reconcile with version-checked diff-merge"   # + trailers
```

---

### Task 4: main-loop wiring — arm + deadline + dispatch

**Files:**
- Modify: `wordcartel/src/app.rs` (arm after the pre-draw rebuild; reconcile deadline; `Msg::Tick` dispatch)

**Interfaces:**
- Consumes: `reconcile::{RECONCILE_DEBOUNCE_MS, reconcile_due, dispatch_reconcile}`; `derive::rebuild`.

**Real anchors:** pre-draw rebuild app.rs:2136; deadline computation 2089–2114 (`diagnostics_run::next_deadline`); `recv_timeout` 2118; diagnostics dispatch inside `Msg::Tick` app.rs:1757–1767.

- [ ] **Step 1: Arm the debounce after the pre-draw rebuild** — immediately AFTER `derive::rebuild(&mut editor);` (app.rs:2136), add (this single site covers active edits AND switch-to-stale — the pre-draw rebuild runs after both, and `armed_for_version` prevents idle-Tick re-arming while re-arming on each new edit):

```rust
        derive::rebuild(&mut editor);
        // Arm the reconcile debounce when the tree is (possibly) stale. Re-arm only
        // when the version advanced since the last arm (so idle Ticks don't push the
        // deadline forever); arm-from-None also covers a switch to a stale buffer.
        {
            let now = clock.now_ms();
            let b = editor.active_mut();
            if b.reconcile.maybe_stale && b.reconcile.in_flight_version.is_none()
                && (b.reconcile.due_at.is_none() || b.reconcile.armed_for_version != b.document.version)
            {
                b.reconcile.due_at = Some(now.saturating_add(crate::reconcile::RECONCILE_DEBOUNCE_MS));
                b.reconcile.armed_for_version = b.document.version;
            }
        }
```

- [ ] **Step 2: Include the reconcile deadline** — in the deadline block (app.rs:2089–2114), add a term (guard on no in-flight, exactly like `diag_deadline` at 2104 to avoid a busy-spin):

```rust
        let reconcile_deadline = if editor.active().reconcile.in_flight_version.is_none() {
            editor.active().reconcile.due_at
        } else {
            None
        };
        let deadline = crate::diagnostics_run::next_deadline(&[
            swap_deadline,
            sq_deadline,
            sb_deadline,
            diag_deadline,
            reconcile_deadline,
        ]);
```

- [ ] **Step 3: Dispatch the reconcile in `Msg::Tick`** — after the diagnostics dispatch (app.rs:1767), add:

```rust
            // Dispatch a block-tree reconcile if due.
            if crate::reconcile::reconcile_due(&editor.active().reconcile, now) {
                crate::reconcile::dispatch_reconcile(editor, ex);
            }
```
(`now` and `ex` are already in scope in the `Msg::Tick` arm; `now = clock.now_ms()` at app.rs:1748.)

- [ ] **Step 4: Write a wiring test** (in `app.rs` tests) — the arming decision is unit-testable without the real loop; assert that after a stale rebuild the arm sets `due_at`, and that an idle re-check with the same version does not push it:

```rust
#[test]
fn reconcile_arm_sets_due_once_and_debounces_on_new_edit() {
    // Simulate the post-rebuild arm block against a ReconcileStore directly.
    let mut s = crate::reconcile::ReconcileStore { maybe_stale: true, ..Default::default() };
    let arm = |s: &mut crate::reconcile::ReconcileStore, now: u64, version: u64| {
        if s.maybe_stale && s.in_flight_version.is_none()
            && (s.due_at.is_none() || s.armed_for_version != version) {
            s.due_at = Some(now + crate::reconcile::RECONCILE_DEBOUNCE_MS);
            s.armed_for_version = version;
        }
    };
    arm(&mut s, 1000, 5);
    assert_eq!(s.due_at, Some(1000 + crate::reconcile::RECONCILE_DEBOUNCE_MS));
    arm(&mut s, 1050, 5); // idle Tick, same version → no push
    assert_eq!(s.due_at, Some(1000 + crate::reconcile::RECONCILE_DEBOUNCE_MS));
    arm(&mut s, 1100, 6); // new edit → re-debounce
    assert_eq!(s.due_at, Some(1100 + crate::reconcile::RECONCILE_DEBOUNCE_MS));
}
```

- [ ] **Step 5: Run + gates + commit** — full `cargo test -p wordcartel -p wordcartel-core` green; `cargo clippy --workspace --all-targets` clean; no fmt.
```bash
git add wordcartel/src/app.rs
git commit -m "feat(app): wire reconcile — debounce arm, deadline, Tick dispatch"   # + trailers
```

---

## Self-Review

**Spec coverage:** §0 rebuild refactor + version memoization → Task 2 ✓; §1 `blocks_maybe_stale` (as `ReconcileStore.maybe_stale`) + instrumented `WidenReason` → Task 1 (store) + Task 2 (set) ✓; §2 self-armed `reconcile_due_at` → Task 1 (`due_at`/`reconcile_due`) + Task 4 (arm) ✓; §3 Executor `JobKind::Reparse` + panic-isolation + `is_stale`/`apply_panic` arms → Task 3 ✓; §4 version-checked diff-merge into derived cache → Task 3 (merge) ✓; convergence theorem → Task 3 test ✓; the perf-fix test → Task 2 test ✓.

**Robustness beyond the spec (recorded for the review):** the parse phase uses incremental ONLY when `version == blocks_version + 1` (one edit behind) — a multi-edit-before-rebuild gap falls to a safe full parse, closing a latent stale-base mismatch the old per-frame full parse used to mask. The merge clears `in_flight_version` unconditionally (even on version-mismatch discard) so a stale reconcile can't wedge the in-flight gate.

**Type consistency:** `ReconcileStore` fields, `reconcile_due(&ReconcileStore, u64) -> bool`, `dispatch_reconcile(&mut Editor, &dyn Executor)`, `rebuild_downstream(&mut Editor)`, `JobKind::Reparse` — used consistently across tasks.

**Placeholder scan:** Task 2 Step 4 says "move the old derive.rs:111–178 verbatim into `rebuild_downstream`" — that body is the existing, working downstream code (fold reconcile + FoldView + layout loop), moved unchanged; the plan cites its exact current lines rather than re-pasting ~65 lines. No logic is left unspecified.
