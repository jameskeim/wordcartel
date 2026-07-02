# M5 follow-ups Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close two M5 leftovers — surface the undo-eviction hint on every active-buffer edit path (via one universal run-loop check + a 2-line core fix), and bound the last three document-sized `fs::read` paths.

**Architecture:** Almost entirely shell-side. Component (a): a single `Editor::note_undo_eviction(pre_id, pre_version)` called once after the sole production `reduce()` in the run loop; `Editor::apply`'s inline hint removed; plus `last_evicted = 0` reset at the top of `History::undo`/`redo` (core). Component (b): a `file::bounded_read_opt(path, limit)` helper applied at the recovery-predicate, skip-unchanged, and `fingerprint()` reads (the last via a metadata fallback so it never returns `None` for a present file).

**Tech Stack:** Rust, `wordcartel` shell + `wordcartel-core`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-01-wordcartel-m5-followups-design.md` (Codex-clean, round 4).
- Gates: `cargo test -p wordcartel -p wordcartel-core` green; `cargo build`/`test --no-run` warning-free; **`cargo clippy --workspace --all-targets` clean (deny gate is LIVE)**; NO `cargo fmt`; house style (em-dash `—`, never `--`; doc-comment public items).
- The hint string is EXACTLY `"Undo history full — oldest dropped"` (em-dash U+2014), single-sourced as `UNDO_EVICTED_HINT`.
- `MAX_OPEN_BYTES = 64 * 1024 * 1024` (`limits.rs:5`, `u64`).
- Over-cap auxiliary reads NEVER surface a user-facing error — each falls back to its existing safe degradation.
- Trailers on every commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

## File Structure

- `wordcartel-core/src/history.rs` — add `self.last_evicted = 0;` to `undo`/`redo`; add a core test.
- `wordcartel/src/editor.rs` — `UNDO_EVICTED_HINT` const + `Editor::note_undo_eviction` helper; strip `Editor::apply`'s inline hint; retarget/extend the hint tests.
- `wordcartel/src/app.rs` — capture `(id, version)` before the run-loop `reduce` (`:2149`) + call the helper after; convert the recovery-predicate read (`:1936`).
- `wordcartel/src/file.rs` — add `bounded_read_opt`; convert the `save_atomic` skip-unchanged read (`:138`); add a helper test.
- `wordcartel/src/save.rs` — convert `fingerprint()` to the metadata fallback (extract `fingerprint_with_limit`); add over-cap + no-churn tests.

---

### Task 1: Component (a) — undo-eviction hint via one universal seam + core reset

**Files:**
- Modify: `wordcartel-core/src/history.rs` (`undo` :99, `redo` :111; test module)
- Modify: `wordcartel/src/editor.rs` (const + helper; `Editor::apply` :646; tests ~:1079)
- Modify: `wordcartel/src/app.rs` (run loop around `:2149`)

**Interfaces:**
- Produces: `Editor::note_undo_eviction(&mut self, pre_id: BufferId, pre_version: u64)`; module const `UNDO_EVICTED_HINT: &str`. (`BufferId(pub u64)` editor.rs:11; `Document.version: u64`; `Buffer.id: BufferId`; `Editor.status: String`; `Editor::active`/`active_mut` editor.rs:417/421.)

- [ ] **Step 1: Core — failing test for the `last_evicted` reset** (in `history.rs` `#[cfg(test)] mod tests`, beside `eviction_keeps_current_consistent_for_undo_redo`):

```rust
    #[test]
    fn undo_and_redo_reset_last_evicted() {
        use crate::buffer::TextBuffer;
        let mut hist = History::default();
        let mut buf = TextBuffer::from_str("");
        let mut sel = Selection::single(0);
        for _ in 0..3 {
            let at = buf.len();
            let cs = ChangeSet::from_ops(vec![Op::Retain(at), Op::Insert("zzz".into())], at);
            sel = hist.commit(Transaction::new(cs), &mut buf, sel.clone());
        }
        hist.evict_to(5);
        assert!(hist.last_evicted > 0, "precondition: eviction happened");
        hist.undo(&mut buf);
        assert_eq!(hist.last_evicted, 0, "undo commits nothing → resets last_evicted");
        hist.last_evicted = 2; // re-arm the transient by hand
        hist.redo(&mut buf);
        assert_eq!(hist.last_evicted, 0, "redo commits nothing → resets last_evicted");
        // Placement proof: a NO-OP undo (nothing to undo) still resets, because the
        // reset precedes the `current == 0` early-return guard.
        let mut h2 = History::default();
        h2.last_evicted = 5;
        h2.undo(&mut buf);
        assert_eq!(h2.last_evicted, 0, "no-op undo still consumes stale eviction state");
    }
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p wordcartel-core undo_and_redo_reset_last_evicted` → FAIL (`last_evicted` still > 0 after undo).

- [ ] **Step 3: Core — insert the reset as the FIRST statement of `undo` and `redo`** (before the early-return guards):

```rust
    pub fn undo(&mut self, buf: &mut TextBuffer) -> Option<Selection> {
        self.last_evicted = 0; // undo commits nothing — keep the eviction transient honest
        if self.current == 0 {
            return None;
        }
        // ... unchanged ...
    }

    pub fn redo(&mut self, buf: &mut TextBuffer) -> Option<Selection> {
        self.last_evicted = 0; // redo commits nothing — keep the eviction transient honest
        if self.current >= self.revisions.len() {
            return None;
        }
        // ... unchanged ...
    }
```

- [ ] **Step 4: Run core tests** — `cargo test -p wordcartel-core` green (new test + `eviction_keeps_current_consistent_for_undo_redo` still passes, since its `last_evicted > 0` assertion precedes its undo/redo).

- [ ] **Step 5: Shell — const + helper** (in `editor.rs`; put the const at module level, e.g. just above the `impl Editor` block):

```rust
const UNDO_EVICTED_HINT: &str = "Undo history full — oldest dropped";
```
and in `impl Editor`:
```rust
    /// Surface the undo-eviction hint iff an edit landed on the STILL-active buffer
    /// this reduce AND it evicted. Consumes `last_evicted` (resets to 0) so a later
    /// undo/redo/switch — which change `version` without a fresh eviction — do not
    /// replay the stale hint. Called once per reduce from the run loop.
    pub fn note_undo_eviction(&mut self, pre_id: BufferId, pre_version: u64) {
        let fire = {
            let b = self.active();
            b.id == pre_id && b.document.version != pre_version
                && b.document.history.last_evicted > 0
        };
        if fire {
            self.status = UNDO_EVICTED_HINT.to_string();
            self.active_mut().document.history.last_evicted = 0;
        }
    }
```

- [ ] **Step 6: Shell — strip `Editor::apply`'s inline hint** (`editor.rs:646`), leaving a plain delegator:

```rust
    pub fn apply(&mut self, txn: Transaction, edit: wordcartel_core::block_tree::Edit, kind: EditKind, clock: &dyn Clock) {
        self.active_mut().apply(txn, edit, kind, clock);
    }
```

- [ ] **Step 7: Shell — failing tests for the helper.** KEEP `apply_does_not_set_hint_when_no_eviction` (~:1078) as a guard that `Editor::apply` itself sets no hint — but REFRESH its stale doc comment (editor.rs:1074-1077 describes the old apply-based hint mechanism removed in Step 6; rewrite it to say the hint now lives in `note_undo_eviction`, and this test guards that `apply` is a pure delegator). Then ADD:

```rust
    #[test]
    fn note_undo_eviction_fires_once_on_active_edit_with_eviction() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        let id = e.active().id;
        let v = e.active().document.version;
        e.active_mut().document.version += 1;              // simulate an edit
        e.active_mut().document.history.last_evicted = 1;  // that evicted
        e.note_undo_eviction(id, v);
        assert_eq!(e.status, UNDO_EVICTED_HINT);
        assert_eq!(e.active().document.history.last_evicted, 0, "consumed");
        // A later version bump (e.g. undo) must NOT replay the stale hint:
        e.status.clear();
        let v2 = e.active().document.version;
        e.active_mut().document.version += 1;
        e.note_undo_eviction(id, v2);
        assert_ne!(e.status, UNDO_EVICTED_HINT, "no re-fire after reset");
    }

    #[test]
    fn note_undo_eviction_ignores_no_edit_and_switch() {
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        let id = e.active().id;
        let v = e.active().document.version;
        e.active_mut().document.history.last_evicted = 1;
        e.note_undo_eviction(id, v);                       // version unchanged → no edit
        assert_ne!(e.status, UNDO_EVICTED_HINT, "no edit → no hint");
        e.active_mut().document.version += 1;
        e.note_undo_eviction(BufferId(id.0.wrapping_add(999)), v); // id mismatch → switch
        assert_ne!(e.status, UNDO_EVICTED_HINT, "id mismatch (switch) → no hint");
    }
```
(`UNDO_EVICTED_HINT` is a private module const, so these tests — in the same module's `#[cfg(test)]` child — reference it via `super::UNDO_EVICTED_HINT` if needed; use the literal string if the `use super::*;` glob doesn't bring in the private const.)

- [ ] **Step 8: Run to verify the new tests fail** — `cargo test -p wordcartel --lib note_undo_eviction` → FAIL (helper not yet wired / compile). Then confirm they pass after Steps 5–6 compile.

- [ ] **Step 9: Wire the run-loop call** (`app.rs`, around `:2149`) — capture before, call after:

```rust
        let (pre_id, pre_version) = { let b = editor.active(); (b.id, b.document.version) };
        let keep = reduce(msg, &mut editor, &reg, &keymap, &executor, &clock, &msg_tx);
        editor.note_undo_eviction(pre_id, pre_version);
        crate::clipboard::drain_clipboard_intents(&mut editor, guard.terminal().backend_mut(), &clip_tx, &msg_tx);
```
(Inserted lines: the `let (pre_id, pre_version)` before `:2149`, and `editor.note_undo_eviction(...)` immediately after the `reduce` line, before the clipboard drain at `:2150`.)

- [ ] **Step 10: Run + gates + commit** — `cargo test -p wordcartel -p wordcartel-core` green; `cargo clippy --workspace --all-targets` clean.
```bash
git add wordcartel-core/src/history.rs wordcartel/src/editor.rs wordcartel/src/app.rs
git commit -m "feat(m5): surface undo-eviction hint on all edit paths via run-loop seam + core last_evicted reset"   # + trailers
```

---

### Task 2: Component (b) — bound the last three document-sized reads

**Files:**
- Modify: `wordcartel/src/file.rs` (`bounded_read_opt`; `save_atomic` :138; helper test)
- Modify: `wordcartel/src/save.rs` (`fingerprint` :30 → `fingerprint_with_limit`; tests)
- Modify: `wordcartel/src/app.rs` (recovery predicate :1936)

**Interfaces:**
- Produces: `file::bounded_read_opt(path: &Path, limit: u64) -> Option<Vec<u8>>`; `save::fingerprint_with_limit(path: &Path, limit: u64) -> Option<FileFingerprint>` (private; `fingerprint` delegates). `MAX_OPEN_BYTES: u64` (limits.rs:5). `FileFingerprint { mtime: Option<SystemTime>, size: u64, hash: u64 }`.

- [ ] **Step 1: `bounded_read_opt` + failing test** (in `file.rs`; `fs`, `std::io::Read`, `Path` already imported at :5-7):

```rust
/// Read `path` fully, capping the allocation at `limit` bytes. Returns `None` if the
/// file exceeds `limit` OR any open/read error occurs — every caller treats `None` as
/// its existing safe degradation. Mirrors `open`'s `.take(limit + 1)` + `len > limit`.
pub fn bounded_read_opt(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    let f = fs::File::open(path).ok()?;
    Read::read_to_end(&mut f.take(limit + 1), &mut buf).ok()?;
    if buf.len() as u64 > limit { return None; }
    Some(buf)
}
```
Test (beside `save_same_content_returns_unchanged`; use the existing `scratch_path` helper):
```rust
    #[test]
    fn bounded_read_opt_caps_allocation() {
        let p = scratch_path("bounded");
        fs::write(&p, b"abc").unwrap();
        assert_eq!(bounded_read_opt(&p, 4).as_deref(), Some(&b"abc"[..]), "3 ≤ limit 4 → Some");
        assert_eq!(bounded_read_opt(&p, 3).as_deref(), Some(&b"abc"[..]), "exactly limit → Some");
        fs::write(&p, b"0123456789").unwrap();
        assert_eq!(bounded_read_opt(&p, 4), None, "10 > limit 4 → None");
        let _ = fs::remove_file(&p);
        assert_eq!(bounded_read_opt(&p, 4), None, "missing path → None");
    }
```

- [ ] **Step 2: Run to verify it fails/compiles** — `cargo test -p wordcartel --lib bounded_read_opt_caps_allocation` → FAIL (not defined) then PASS after Step 1 compiles.

- [ ] **Step 3: Convert `save_atomic` skip-unchanged** (`file.rs:138`):

```rust
    // (2) Skip-unchanged — bounded read; over-cap or unreadable → skip the optimization.
    if let Some(existing) = bounded_read_opt(path, crate::limits::MAX_OPEN_BYTES) {
        if existing == content.as_bytes() {
            return Ok(SaveOutcome::Unchanged);
        }
    }
```
(Confirm `save_same_content_returns_unchanged` at :217 still passes — a small identical file reads back within cap → `Some` → `Unchanged`.)

- [ ] **Step 4: Convert `fingerprint()` to the metadata fallback + failing tests** (`save.rs:30`):

Write a FRESH doc comment for the reshaped public `fingerprint` (do NOT keep the
old one verbatim — its "hash and size come from the same read (no TOCTOU)" claim is
now false, since `size` comes from `meta.len()` (a stat) and `hash` from a separate
bounded read):
```rust
/// Content-hash fingerprint of `path` for external-modification detection (BUG-2),
/// capping the content read at `MAX_OPEN_BYTES`. Returns `None` only when `path` is
/// missing/unstattable; a present file always yields `Some` (over-cap → mtime+size
/// with a sentinel hash — never `None`, so `stored_fp` can't silently disable the
/// conflict check). `mtime`/`size` come from `metadata`, `hash` from a separate
/// bounded read (no single-syscall guarantee across the three fields).
pub fn fingerprint(path: &Path) -> Option<FileFingerprint> {
    fingerprint_with_limit(path, crate::limits::MAX_OPEN_BYTES)
}

/// Content-hash fingerprint, capping the content read at `limit`. `meta` failure
/// (missing/unreadable) → `None`; but a present-but-over-cap file yields a
/// metadata-only fingerprint (real mtime+size, sentinel hash 0) rather than `None`,
/// so `stored_fp` never becomes `None` and the external-mod check is not silently
/// defeated (`None == None`) for files grown beyond the cap.
fn fingerprint_with_limit(path: &Path, limit: u64) -> Option<FileFingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    let hash = match crate::file::bounded_read_opt(path, limit) {
        Some(bytes) => {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            std::hash::Hasher::write(&mut h, &bytes);
            std::hash::Hasher::finish(&h)
        }
        None => 0, // over-cap (or transient read failure): fall back to mtime+size only
    };
    Some(FileFingerprint { mtime: meta.modified().ok(), size: meta.len(), hash })
}
```
Tests (beside `fingerprint_detects_same_size_different_content`; use the existing `scratch` helper):
```rust
    #[test]
    fn fingerprint_over_cap_falls_back_to_metadata_not_none() {
        let p = scratch();
        std::fs::write(&p, b"0123456789").unwrap(); // 10 bytes
        let fp = fingerprint_with_limit(&p, 4).expect("over-cap present file still yields a fingerprint");
        assert_eq!(fp.size, 10, "size from metadata");
        assert_eq!(fp.hash, 0, "over-cap → sentinel hash, NOT None (closes None==None)");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn fingerprint_within_cap_hashes_content_unchanged() {
        let p = scratch();
        std::fs::write(&p, b"aaaa").unwrap();
        let within = fingerprint_with_limit(&p, 1_000_000).expect("fp");
        assert_ne!(within.hash, 0, "≤cap → real content hash");
        assert_eq!(within.hash, fingerprint(&p).unwrap().hash, "≤cap path identical to public fingerprint (no churn)");
        let _ = std::fs::remove_file(&p);
    }
```

- [ ] **Step 5: Run to verify** — `cargo test -p wordcartel --lib fingerprint` → the two new tests PASS; `fingerprint_detects_same_size_different_content` (:683) and `fingerprint_matrix_new_and_deleted_are_conflicts` (:449) still PASS (small files: `meta.len() == bytes.len()`, real hash unchanged).

- [ ] **Step 6: Convert the recovery predicate** (`app.rs:1936`):

```rust
        // Bounded read: an over-cap document yields None → assess() Prompts (safe).
        // (Narrow behavior change: a >64 MiB file whose bytes match the swap hash
        // would previously DiscardSilently; it now Prompts. Safe direction.)
        let file_bytes = editor.active().document.path.as_deref()
            .and_then(|p| crate::file::bounded_read_opt(p, crate::limits::MAX_OPEN_BYTES));
```
(`file_bytes` stays `Option<Vec<u8>>`; the downstream `file_bytes.as_deref()` into `swap::assess` is unchanged. Over-cap → `None` → `assess` returns `Prompt`.)

- [ ] **Step 7: Run + gates + commit** — full `cargo test -p wordcartel -p wordcartel-core` green; `cargo clippy --workspace --all-targets` clean.
```bash
git add wordcartel/src/file.rs wordcartel/src/save.rs wordcartel/src/app.rs
git commit -m "feat(m5): bound the last 3 document-sized reads (bounded_read_opt + fingerprint metadata fallback)"   # + trailers
```

---

## Self-Review

**Spec coverage:** (a) single run-loop seam → Task 1 Steps 5,9 ✓; remove `Editor::apply` hint → Step 6 ✓; core `last_evicted` reset (before the guards) → Steps 1-3 ✓; single-source hint const → Step 5 ✓. (b) `bounded_read_opt` → Task 2 Step 1 ✓; recovery predicate → Step 6 ✓; skip-unchanged → Step 3 ✓; `fingerprint` metadata fallback (no `None==None`) → Step 4 ✓. Testing: helper unit tests (fresh, id/version/reset gates), core reset test (incl. no-op placement proof), `bounded_read_opt` tiny-limit test, `fingerprint` over-cap + no-churn tests, all named regression tests preserved.

**Placeholder scan:** none — every step has complete code.

**Type consistency:** `note_undo_eviction(pre_id: BufferId, pre_version: u64)` matches `Buffer.id: BufferId` + `Document.version: u64`; `bounded_read_opt(&Path, u64) -> Option<Vec<u8>>` matches all three call sites; `fingerprint_with_limit(&Path, u64) -> Option<FileFingerprint>` with `size: meta.len()` (u64) and `hash: u64`.

**Ordering note:** Task 1 Step 9 places the helper call AFTER `reduce` returns (so it overrides the outcome status) and reads the active buffer that reduce left active — the `(id, version)` gate handles the edit-then-switch and no-edit cases. The documented residual gaps (edit+switch in one reduce; inactive eviction not announced) are accepted per spec.
