# M5 — Resource Caps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bound every unbounded memory/work path (document open, undo history, search matches, transform/filter output, scratch/session, swap recovery) with fixed, centrally-audited caps — so a pathological input cannot OOM/hang/freeze the UI, and M7's fuzz CI is safe.

**Architecture:** A central `wordcartel/src/limits.rs` holds every shell-side quota as a `const`; core's undo cap lives in `wordcartel-core`. One 64 MiB master document bound unifies open/filter/transform. Refuse on input/output edges (open, transform); degrade on caches (undo bytes w/ keep-latest + loud hint, search matches, session). Every load path is bounded at read-time via `Read::take`, not by trusting metadata.

**Tech Stack:** Rust. `std::io::Read::take` for bounded reads. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-29-wordcartel-m5-resource-caps-design.md` (Codex-reviewed ×2, GO).

## Global Constraints

- **Behavior-preserving except three deliberate changes:** (1) filter output cap 1 MiB → 64 MiB; (2) new refusals (`OpenError::TooLarge`, `TransformError::OutputTooLarge`); (3) undo history now bounded (evicts oldest past 64 MiB, keeping ≥1). Everything else (normal editing, normal-size files) behaves identically.
- **Values:** `MAX_OPEN_BYTES = MAX_FILTER_OUTPUT = MAX_TRANSFORM_OUTPUT = MAX_UNDO_BYTES = 64 * 1024 * 1024`; `MAX_SEARCH_MATCHES = 100_000`; `MAX_SESSION_BYTES = 8 * 1024 * 1024`; paste consts unchanged (`8 MiB` / `100_000`).
- **`ChangeSet` fields are private (M1)** — add methods, never reach into `ops`.
- **Gates (corrected M5 gates — NOT the old ones):** `cargo test -p wordcartel-core -p wordcartel` green; `cargo build` + `cargo test --no-run` warning-free for touched crates; **no new clippy findings on touched lines** (`cargo clippy -p <crate> --tests`, check your diff — do NOT chase a clean whole-workspace `-D warnings`; the repo has pre-existing clippy debt that is out of scope). **Do NOT run `cargo fmt`** — hand-formatted dense house style, no rustfmt.toml; match neighbors by hand (`—` em-dashes never `--`, single-line blocks where they read well, no emoji in code).
- Commit trailers (append to every commit message):
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
  `Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6`

## File Structure

- **Create** `wordcartel/src/limits.rs` — central shell consts (responsibility: the one auditable place for "we are bounded here").
- **Modify** `wordcartel/src/lib.rs` — `pub mod limits;` (one line).
- **Modify** `wordcartel-core/src/history.rs` — `MAX_UNDO_BYTES`, `History.{bytes,last_evicted}`, eviction in `commit`/`commit_coalescing`.
- **Modify** `wordcartel-core/src/change.rs` — `ChangeSet::stored_bytes`.
- **Modify** `wordcartel-core/src/search.rs` — `all_matches` gains a `limit`.
- **Modify** `wordcartel/src/{file,transform,search_overlay,state,swap,clipboard,app,editor}.rs` — the per-site caps.

---

### Task 1: `limits.rs` foundation + paste re-home + filter cap raise

**Files:**
- Create: `wordcartel/src/limits.rs`
- Modify: `wordcartel/src/lib.rs` (`pub mod limits;`), `wordcartel/src/clipboard.rs` (re-export), `wordcartel/src/app.rs:668-675` (filter builder), `wordcartel-core/src/history.rs` (the `MAX_UNDO_BYTES` const only)
- Test: `wordcartel/src/limits.rs` + an `app.rs`/`filter.rs` filter-raise test

**Interfaces:**
- Produces: `wordcartel::limits::{MAX_OPEN_BYTES: u64, MAX_FILTER_OUTPUT: usize, MAX_TRANSFORM_OUTPUT: usize, MAX_SEARCH_MATCHES: usize, MAX_SESSION_BYTES: usize, PASTE_MAX_BYTES: usize, OSC52_MAX_ENCODED: usize}`; `wordcartel_core::history::MAX_UNDO_BYTES: usize`.

- [ ] **Step 1: Create `limits.rs`**

```rust
//! Central resource quotas (M5). The one auditable place for "the program is bounded
//! here". Fixed safety rails — refuse on input/output edges, degrade on caches.

/// Refuse opening a file larger than this (enforced by a bounded read, not just metadata).
pub const MAX_OPEN_BYTES: u64 = 64 * 1024 * 1024;
/// Ceiling on a single filter subprocess's output (raised from 1 MiB so a whole-document
/// filter on a large doc is not spuriously refused).
pub const MAX_FILTER_OUTPUT: usize = 64 * 1024 * 1024;
/// Ceiling on a single in-process transform's output.
pub const MAX_TRANSFORM_OUTPUT: usize = 64 * 1024 * 1024;
/// Stop collecting search matches past this (bounds the "everything matches" scan + vector).
pub const MAX_SEARCH_MATCHES: usize = 100_000;
/// Skip/trim persisting a serialized session larger than this; bound the load read at it too.
pub const MAX_SESSION_BYTES: usize = 8 * 1024 * 1024;

/// Max decoded paste size (canonical home; re-exported from clipboard.rs).
pub const PASTE_MAX_BYTES: usize = 8 * 1024 * 1024;
/// Max OSC-52 encoded clipboard payload (canonical home; re-exported from clipboard.rs).
pub const OSC52_MAX_ENCODED: usize = 100_000;
```

- [ ] **Step 2: Wire the module**

`wordcartel/src/lib.rs` — add near the other IO modules (after `pub mod file;`, keep house ordering, do NOT re-sort):
```rust
pub mod limits;
```

- [ ] **Step 3: Re-home paste consts (zero call-site churn)**

In `wordcartel/src/clipboard.rs`, REPLACE the two local `const` definitions (lines 7-8):
```rust
pub const OSC52_MAX_ENCODED: usize = 100_000;
pub const PASTE_MAX_BYTES: usize = 8 * 1024 * 1024;
```
with a re-export:
```rust
pub use crate::limits::{OSC52_MAX_ENCODED, PASTE_MAX_BYTES};
```
Existing `crate::clipboard::PASTE_MAX_BYTES` references (`app.rs:699-703`, `:2443-2444`) keep compiling.

- [ ] **Step 4: Add the core undo const (value only this task)**

In `wordcartel-core/src/history.rs`, near `COALESCE_MS`:
```rust
/// Undo-history memory budget: evict oldest revisions past this, always keeping ≥1 (M5).
pub const MAX_UNDO_BYTES: usize = 64 * 1024 * 1024;
```
(The eviction logic that USES it lands in Task 2; a lone `pub const` is not dead code.)

- [ ] **Step 5: Raise the filter cap (write the failing test first)**

The single production builder is `app.rs:668-675` (`max_output: 1 << 20`). Add a test that a filter
emitting > 1 MiB but < 64 MiB now succeeds (it previously failed). In `filter.rs` tests, near the
existing cap tests (`filter.rs:440-486`), add:

```rust
#[test]
fn filter_output_above_old_1mib_cap_succeeds_under_new_cap() {
    // Emit ~2 MiB through `cat`; with MAX_FILTER_OUTPUT (64 MiB) this must NOT hit the cap.
    let input = "x".repeat(2 * 1024 * 1024);
    let spec = FilterSpec {
        argv: vec!["cat".into()],
        shell: false,
        disposition: Disposition::Filter,
        input: Input::SelectionElseBuffer,
        timeout: std::time::Duration::from_secs(10),
        max_output: crate::limits::MAX_FILTER_OUTPUT,
    };
    // Real harness (mirrors the neighboring cap tests at filter.rs:431): run_filter(&spec, input, &cancel).
    let out = run_filter(&spec, &input, &CancelFlag::new());
    assert!(out.is_ok(), "2 MiB output must succeed under the 64 MiB cap");
    assert_eq!(out.unwrap().len(), input.len());
}
```
Confirm the exact `run_filter` signature + `CancelFlag` ctor against the neighboring `max_output: 64`
cap tests (filter.rs:431); mirror their call shape exactly. The point is that the spec uses
`MAX_FILTER_OUTPUT` (64 MiB), so 2 MiB output no longer trips the cap.

- [ ] **Step 6: Re-point the production builder**

`app.rs:674`: `max_output: 1 << 20,` → `max_output: crate::limits::MAX_FILTER_OUTPUT,`. Leave the
test-only `max_output: 64` caps (`filter.rs:440-455`, `:460-486`) unchanged.

- [ ] **Step 7: Gates + commit**

`cargo test -p wordcartel --lib filter` green; `cargo build -p wordcartel`/`cargo build -p wordcartel-core` warning-free; `cargo clippy -p wordcartel --tests` — no findings on your new lines.
```bash
git add wordcartel/src/limits.rs wordcartel/src/lib.rs wordcartel/src/clipboard.rs wordcartel/src/app.rs wordcartel-core/src/history.rs
git commit -m "feat(m5): central limits module + paste re-home + filter cap 1MiB->64MiB"
```

---

### Task 2: Undo byte budget (core) + eviction hint (shell)

**Files:**
- Modify: `wordcartel-core/src/change.rs` (`ChangeSet::stored_bytes`), `wordcartel-core/src/history.rs` (`History.{bytes,last_evicted}`, `revision_bytes`, eviction in `commit`/`commit_coalescing`), `wordcartel/src/editor.rs:635` (`Editor::apply` hint)
- Test: `history.rs` `#[cfg(test)] mod tests`; a shell test for the hint

**Interfaces:**
- Consumes: `MAX_UNDO_BYTES` (Task 1).
- Produces: `ChangeSet::stored_bytes(&self) -> usize`; `History.{bytes: usize, last_evicted: usize}`.

- [ ] **Step 1: `ChangeSet::stored_bytes` — failing test first**

In `change.rs` tests:
```rust
#[test]
fn stored_bytes_counts_insert_payload_only() {
    // retain 3, insert "hello" (5), delete 2 -> stored = 5 (only the Insert text).
    let cs = ChangeSet::from_ops(vec![Op::Retain(3), Op::Insert("hello".into()), Op::Delete(2)], 5);
    assert_eq!(cs.stored_bytes(), 5);
}
```
Run → fails (no method). Implement on `ChangeSet`:
```rust
/// Heap text this changeset holds: the sum of `Insert` payload byte lengths. `Retain`/`Delete`
/// are counts (negligible structural overhead, excluded). A revision's true memory is captured
/// by summing this over both its `changes` and `inverse` (a delete's text lives in the inverse).
pub fn stored_bytes(&self) -> usize {
    self.ops.iter().map(|op| match op {
        Op::Insert(s) => s.len(),
        Op::Retain(_) | Op::Delete(_) => 0,
    }).sum()
}
```

- [ ] **Step 2: `History` byte fields + `revision_bytes` helper**

Add fields to `History` (`history.rs:50-54`):
```rust
#[derive(Clone, Debug, Default)]
pub struct History {
    pub revisions: Vec<Revision>,
    pub current: usize,    // number of revisions currently applied
    pub bytes: usize,      // running total of retained revisions' stored bytes (M5)
    pub last_evicted: usize, // revisions dropped on the most recent commit (M5)
}
```
Add a free fn (or `impl` method) near the impl:
```rust
fn revision_bytes(rev: &Revision) -> usize {
    rev.edits.iter().map(|e| e.changes.stored_bytes() + e.inverse.stored_bytes()).sum()
}
```

- [ ] **Step 3: Eviction helper (budget-parameterized for fast tests) + the `current`-invariant test**

The eviction loop takes the budget as a parameter so tests can force eviction with TINY revisions
and a tiny budget (forcing eviction against the real 64 MiB const would mean allocating >64 MiB per
test). Production calls it with `MAX_UNDO_BYTES`.
```rust
impl History {
    /// Evict oldest revisions while over `budget`, keeping ≥1. Sets `last_evicted`. MUST run only
    /// right after a commit (where `current == revisions.len()`), so decrementing `current` per
    /// front-eviction keeps undo/redo indices valid.
    fn evict_to(&mut self, budget: usize) {
        self.last_evicted = 0;
        while self.bytes > budget && self.revisions.len() > 1 {
            let rev = self.revisions.remove(0);
            self.bytes = self.bytes.saturating_sub(revision_bytes(&rev));
            self.current = self.current.saturating_sub(1);
            self.last_evicted += 1;
        }
    }
}
```
Test the load-bearing index fix with small data (commit 3 tiny revisions, then force eviction via a
tiny budget directly):
```rust
#[test]
fn eviction_keeps_current_consistent_for_undo_redo() {
    use crate::buffer::TextBuffer;
    let mut hist = History::default();
    let mut buf = TextBuffer::from_str("");
    let mut sel = Selection::single(0);
    for _ in 0..3 {
        let at = buf.len();
        let cs = ChangeSet::from_ops(vec![Op::Retain(at), Op::Insert("zzz".into())], at);
        sel = hist.commit(Transaction::new(cs), &mut buf, sel.clone());
    }
    // 3 revisions, stored_bytes 3 each (the "zzz" insert) → bytes == 9. Force eviction to ≤5.
    hist.evict_to(5);
    assert!(hist.last_evicted > 0, "oldest revisions must have been evicted");
    assert!(hist.revisions.len() >= 1, "keep at least one");
    assert_eq!(hist.current, hist.revisions.len(), "current must equal len after evict");
    // The critical invariant: undo then redo round-trips without panicking / mis-indexing.
    let pre = buf.to_string();
    hist.undo(&mut buf);
    hist.redo(&mut buf);
    assert_eq!(buf.to_string(), pre, "undo+redo round-trips after eviction");
}

#[test]
fn single_over_budget_revision_is_retained() {
    use crate::buffer::TextBuffer;
    let mut hist = History::default();
    let mut buf = TextBuffer::from_str("");
    let cs = ChangeSet::from_ops(vec![Op::Insert("hello".into())], 0);
    hist.commit(Transaction::new(cs), &mut buf, Selection::single(0));
    hist.evict_to(0); // budget 0, but keep-≥1 means the lone revision stays
    assert_eq!(hist.revisions.len(), 1);
    assert_eq!(hist.last_evicted, 0);
}
```

- [ ] **Step 4: Wire byte accounting into `commit` (with redo-tail subtraction)**

Rewrite `commit` (`history.rs:59-77`) so the tail bytes are subtracted before truncate, the new
revision's bytes added, then evict:
```rust
pub fn commit(&mut self, txn: Transaction, buf: &mut TextBuffer, before: Selection) -> Selection {
    let inverse = txn.changes.invert(buf);
    txn.changes.apply(buf);
    let after = txn.selection.clone().unwrap_or_else(|| before.map(&txn.changes));
    // Drop the redo tail — subtract its bytes FIRST so `bytes` stays accurate.
    let tail: usize = self.revisions[self.current..].iter().map(revision_bytes).sum();
    self.bytes = self.bytes.saturating_sub(tail);
    self.revisions.truncate(self.current);
    let rev = Revision {
        edits: vec![Edit { changes: txn.changes, inverse }],
        before, after: after.clone(), last_ms: 0, kind: EditKind::Other,
    };
    self.bytes += revision_bytes(&rev);
    self.revisions.push(rev);
    self.current += 1;
    self.evict_to(MAX_UNDO_BYTES);
    after
}
```

- [ ] **Step 5: Wire byte accounting into `commit_coalescing` (merge recompute)**

Rewrite the body after `after` is computed (`history.rs:127-143`):
```rust
    if can_merge {
        let (old, new);
        {
            let top = self.revisions.last_mut().unwrap();
            old = revision_bytes(top);
            top.edits.push(Edit { changes: txn.changes, inverse });
            top.after = after.clone();
            top.last_ms = now;
            new = revision_bytes(top);
        }
        self.bytes = self.bytes - old + new; // subtract-then-add avoids any underflow path
    } else {
        let tail: usize = self.revisions[self.current..].iter().map(revision_bytes).sum();
        self.bytes = self.bytes.saturating_sub(tail);
        self.revisions.truncate(self.current);
        let rev = Revision {
            edits: vec![Edit { changes: txn.changes, inverse }],
            before, after: after.clone(), last_ms: now, kind,
        };
        self.bytes += revision_bytes(&rev);
        self.revisions.push(rev);
        self.current += 1;
    }
    self.evict_to(MAX_UNDO_BYTES);
    after
```

- [ ] **Step 6: Core tests — accounting + keep-latest + truncation**

Add tests: `commit` then a redo-tail-truncating commit leaves `bytes` equal to a fresh recompute
(`hist.revisions.iter().map(revision_bytes).sum()`); a single over-budget revision is retained
(`len()==1`, `last_evicted` counts the rest); a normal small-edit session never evicts
(`last_evicted==0`, `bytes` small). Expose `revision_bytes` to the test module (it's in the same
file) or assert via `hist.bytes`.

Run: `cargo test -p wordcartel-core --lib history change` → all green.

- [ ] **Step 7: Shell hint in `Editor::apply` (scoped) — failing test first**

`Editor::apply` (`editor.rs:635`) is the command/typing edit path (called from commands.rs) and owns
`self.status`; it delegates to the active buffer's `Buffer::apply` (which runs `commit_coalescing`).
**Scope note:** `Editor::apply` is NOT the universal mutation choke point — `transform`/`filter` merges
call `Buffer::apply` (editor.rs:179) directly, bypassing it. We scope the hint to `Editor::apply` on
purpose: sustained TYPING (the coalescing path) is what realistically accumulates 64 MiB of undo and
triggers eviction; a single transform/filter merge that happens to evict simply won't show the hint
(the eviction itself still happens correctly — `last_evicted` is set, just not surfaced). This is an
acceptable scope for a polish hint; do NOT duplicate the hook into `Buffer::apply` (which has no access
to `Editor.status`).

After the inner apply runs inside `Editor::apply`:
```rust
if self.active().document.history.last_evicted > 0 {
    self.status = "Undo history full — oldest dropped".to_string();
}
```
Shell test (in `editor.rs` tests): the core eviction test already proves `last_evicted` is set; here
just prove the WIRING — force a small eviction through `Editor::apply` (commit a few tiny edits, but
the const budget makes a real eviction heavy). Keep it fast: assert that after a NON-evicting edit
`editor.status` does NOT get the hint (i.e. the guard `last_evicted > 0` is respected), and rely on the
core test for the eviction-true branch. If a fast eviction-true shell assertion is feasible, add it;
otherwise document that the true-branch is covered by the core `last_evicted` test.

- [ ] **Step 8: Gates + commit**

`cargo test -p wordcartel-core -p wordcartel` green; warning-free build; clippy clean on touched lines.
```bash
git add wordcartel-core/src/change.rs wordcartel-core/src/history.rs wordcartel/src/editor.rs
git commit -m "feat(m5): bound undo history by bytes (keep-latest, current-safe eviction) + hint"
```

---

### Task 3: Document-open size cap (`file.rs`)

**Files:**
- Modify: `wordcartel/src/file.rs` (`OpenError` enum, `open`)
- Test: `file.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::limits::MAX_OPEN_BYTES`.
- Produces: `OpenError::TooLarge(String, u64)` (label, limit); `open` signature unchanged.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn open_refuses_file_over_cap() {
    let p = scratch_path("toobig");
    // Create a sparse file just over the cap WITHOUT writing 64 MiB (fast): set_len.
    let f = std::fs::File::create(&p).unwrap();
    f.set_len(crate::limits::MAX_OPEN_BYTES + 1).unwrap();
    drop(f);
    let err = open(&p).expect_err("must refuse oversized file");
    assert!(matches!(err, OpenError::TooLarge(..)), "expected TooLarge, got {err:?}");
    let _ = std::fs::remove_file(&p);
}
```
(Also keep a positive test: a small file still opens; the existing `save_and_open_roundtrip` covers it.)

- [ ] **Step 2: Add the variant**

```rust
#[derive(thiserror::Error, Debug)]
pub enum OpenError {
    // ... existing NotFound/Binary/Permission/IsDir/Io ...
    #[error("{0}: too large (> {1} bytes)")]
    TooLarge(String, u64),
}
```
(Tuple form keeps the Display simple and matches the existing `{0}`-style variants; carry `label`
and `limit`. The exact-size is optional — the metadata fast-path knows it, the bounded-read tripwire
does not, so the message uses the limit.)

- [ ] **Step 3: Bounded read in `open`**

Replace the `fs::read(path)` slurp (`file.rs:60`) with a metadata fast-check + bounded read:
```rust
pub fn open(path: &Path) -> Result<String, OpenError> {
    let label = path.display().to_string();
    let limit = crate::limits::MAX_OPEN_BYTES;
    // (a) Fast refusal when metadata is trustworthy.
    if let Ok(meta) = fs::metadata(path) {
        if meta.is_file() && meta.len() > limit {
            return Err(OpenError::TooLarge(label, limit));
        }
    }
    // (b) Bounded read — caps the allocation even if metadata lied (/proc, sparse).
    let bytes = match fs::File::open(path) {
        Ok(f) => {
            let mut buf = Vec::new();
            if let Err(e) = std::io::Read::take(f, limit + 1).read_to_end(&mut buf) {
                return Err(map_open_io_err(e, &label, path)); // factor the existing kind-mapping
            }
            if buf.len() as u64 > limit {
                return Err(OpenError::TooLarge(label, limit));
            }
            buf
        }
        Err(e) => return Err(map_open_io_err(e, &label, path)),
    };
    // ... existing is_dir / is_binary(NUL+utf8) / String::from_utf8 cascade on `bytes` ...
}
```
Extract the existing `match e.kind() { NotFound/PermissionDenied/_ => IsDir-or-Io }` logic
(`file.rs:63-84`) verbatim into a small `fn map_open_io_err(e: std::io::Error, label: &str, path: &Path) -> OpenError`
reused by both the `File::open` and `read_to_end` error sites. It MUST preserve BOTH `is_dir()`
disambiguation sites — the one in the `NotFound` arm (a dir can surface as NotFound on some FS) AND the
one in the catch-all `_` arm — exactly as today, plus the post-read `if path.is_dir()` check and the
`is_binary`/`from_utf8` cascade. `read_to_end` needs `use std::io::Read;` in scope.

- [ ] **Step 4: Verify the existing open tests still pass + new one**

Run: `cargo test -p wordcartel --lib file::tests` — `save_and_open_roundtrip`, `open_nul_byte_returns_binary`, `open_missing_returns_not_found`, the new `open_refuses_file_over_cap` all green (the error-kind mapping must be preserved by `map_open_io_err`).

- [ ] **Step 5: Gates + commit**

```bash
git add wordcartel/src/file.rs
git commit -m "feat(m5): refuse opening files over MAX_OPEN_BYTES (bounded read, not just metadata)"
```

---

### Task 4: Search match cap (core `all_matches` + shell)

**Files:**
- Modify: `wordcartel-core/src/search.rs` (`all_matches` + its in-module test calls), `wordcartel/src/search_overlay.rs` (pass limit, store `capped`, status)
- Test: `search.rs` (cap behavior) + `search_overlay.rs` (capped flag)

**Interfaces:**
- Consumes: `crate::limits::MAX_SEARCH_MATCHES`.
- Produces: `all_matches(rope: &Rope, m: &Matcher, limit: usize) -> (Vec<Match>, bool)` — at most
  `limit` matches, plus a `capped` flag that is `true` ONLY when a match beyond `limit` exists
  (exact: returns `false` when exactly `limit` matches exist).

- [ ] **Step 1: Failing test (core)**

`Matcher` is built via `compile` (search.rs:35) — there is no `Matcher::literal`. Use the real ctor:
```rust
#[test]
fn all_matches_stops_at_limit_and_reports_capped() {
    let rope = Rope::from_str(&"a ".repeat(1000)); // 1000 "a" matches
    let m = compile("a", QueryMode::Literal, CaseMode::Sensitive).unwrap();
    let (got, capped) = all_matches(&rope, &m, 10);
    assert_eq!(got.len(), 10, "must stop collecting at the limit");
    assert!(capped, "more than 10 matches exist → capped");

    let (all, capped2) = all_matches(&rope, &m, usize::MAX);
    assert_eq!(all.len(), 1000);
    assert!(!capped2, "collecting everything is not capped");
}
```
(Confirm the exact `QueryMode`/`CaseMode` variant names against search.rs:35; the call shape is the
real `compile(needle, mode, case)`.)

- [ ] **Step 2: Add the `limit` param + exact `capped` detection**

`all_matches` (`search.rs:53`) gains `limit: usize` and returns `(matches, capped)`. The `capped`
flag is exact — set only when a `(limit+1)`-th match is actually found (so exactly `limit` matches
is NOT capped):
```rust
pub fn all_matches(rope: &Rope, m: &Matcher, limit: usize) -> (Vec<Match>, bool) {
    let mut out = Vec::new();
    let mut at = 0usize;
    let end = rope.len_bytes();
    let mut capped = false;
    loop {
        match next_from(rope, m, at) {
            Some(mm) => {
                if out.len() == limit {
                    // We already hold `limit` matches and another exists → capped; don't push it.
                    capped = true;
                    break;
                }
                let advance = if mm.end > mm.start { mm.end } else { next_boundary(rope, mm.end) };
                out.push(mm);
                if advance > end || advance <= at { break; }
                at = advance;
            }
            None => break,
        }
    }
    (out, capped)
}
```

- [ ] **Step 3: Update the in-module test call sites**

`search.rs:209/217/224/231/251/266/278/287/295` call `all_matches(&rope, &m)` — change each to
`all_matches(&rope, &m, usize::MAX).0` (these tests assert on the match vec; `usize::MAX` preserves
the unlimited behavior and `.0` selects the vec). Run them to confirm green.

- [ ] **Step 4: Shell — pass the cap + record `capped`**

`search_overlay.rs:100`:
```rust
let (matches, capped) = search::all_matches(rope, &m, crate::limits::MAX_SEARCH_MATCHES);
self.matches = matches;
self.capped = capped;
self.matcher = Some(m);
```
Add a `capped: bool` field to `SearchState`'s cache (near `matches: Vec<Match>`, search_overlay.rs:26),
default `false`, and reset it on every recompute (the `recompute` paths that clear `matches` must also
clear `capped`). Where the search echo/status is composed, append `" (first 100000)"` (or similar)
when `capped`. Navigation is unchanged.

- [ ] **Step 5: Shell test**

A `search_overlay` test: feed a buffer with a feasible match count and a small enough document, assert
`matches.len() == MAX_SEARCH_MATCHES` would require 100k matches (slow) — so instead assert the core
contract directly (Step 1 covers the cap+capped logic) and add a lightweight shell test that
`SearchState.capped` is wired from the returned flag (e.g. via a small helper or by checking that a
recompute over a tiny buffer leaves `capped == false`). Keep it fast.

- [ ] **Step 6: Gates + commit**

`cargo test -p wordcartel-core --lib search` + `cargo test -p wordcartel --lib search_overlay` green.
```bash
git add wordcartel-core/src/search.rs wordcartel/src/search_overlay.rs
git commit -m "feat(m5): cap search match collection at MAX_SEARCH_MATCHES (in-collector)"
```

---

### Task 5: Session caps (`app.rs` scratch guard + `state.rs` save/load)

**Files:**
- Modify: `wordcartel/src/app.rs` (~:2118-2128 scratch snapshot), `wordcartel/src/state.rs` (`save_in`, `load_in`)
- Test: `state.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::limits::MAX_SESSION_BYTES`.

- [ ] **Step 1: Failing tests (state)**

```rust
#[test]
fn save_in_drops_oversized_scratch_keeps_metadata() {
    let d = tmp();
    let mut s = SessionState::default();
    s.entries.insert("/a".into(), /* a small StateEntry */);
    s.scratch = Some(ScratchState { text: "x".repeat(crate::limits::MAX_SESSION_BYTES + 1), cursor: 0 });
    s.save_in(&d).unwrap();
    let back = load_in(&d);
    assert!(back.scratch.is_none(), "oversized scratch dropped");
    assert!(back.entries.contains_key("/a"), "metadata still persisted");
}

#[test]
fn load_in_over_cap_returns_empty() {
    let d = tmp();
    std::fs::write(d.join("session.toml"), "x".repeat(crate::limits::MAX_SESSION_BYTES + 1)).unwrap();
    assert!(load_in(&d).entries.is_empty(), "over-cap session.toml → empty");
}
```

- [ ] **Step 2: `save_in` — drop oversized scratch**

```rust
pub fn save_in(&self, dir: &Path) -> std::io::Result<()> {
    let mut text = toml::to_string(self).map_err(ser_err)?;
    if text.len() > crate::limits::MAX_SESSION_BYTES {
        let trimmed = SessionState { scratch: None, ..self.clone() };
        text = toml::to_string(&trimmed).map_err(ser_err)?;
        if text.len() > crate::limits::MAX_SESSION_BYTES {
            return Ok(()); // metadata alone over cap (shouldn't happen) → skip persist
        }
    }
    crate::file::save_atomic_bytes(&dir.join("session.toml"), text.as_bytes())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
}
```
(`ser_err` = the existing `|e| io::Error::new(Other, format!("session serialize: {e}"))` closure.)

- [ ] **Step 3: `load_in` — bounded read**

```rust
pub fn load_in(dir: &Path) -> SessionState {
    let path = dir.join("session.toml");
    let Ok(f) = std::fs::File::open(&path) else { return SessionState::default() };
    let mut text = String::new();
    let cap = crate::limits::MAX_SESSION_BYTES as u64;
    if std::io::Read::take(f, cap + 1).read_to_string(&mut text).is_err()
        || text.len() as u64 > cap {
        return SessionState::default(); // unreadable or over-cap → empty (graceful)
    }
    toml::from_str(&text).unwrap_or_default()
}
```
(`use std::io::Read;` in scope.)

- [ ] **Step 4: `app.rs` scratch snapshot — guard before `to_string()`**

At the scratch capture (`app.rs:2118-2128`), check the buffer byte length first so a huge scratch is
never materialized into a `String`. `TextBuffer::len()` returns `rope.len_bytes()` (buffer.rs:15) —
use `len()`, NOT `len_bytes()` (no such method on the buffer):
```rust
if let Some(sid) = editor.scratch_id {
    if let Some(sb) = editor.by_id(sid) {
        if sb.document.buffer.len() <= crate::limits::MAX_SESSION_BYTES {
            session.scratch = Some(crate::state::ScratchState {
                text: sb.document.buffer.to_string(),
                cursor: sb.document.selection.primary().head,
            });
        } // else: leave session.scratch = None (live buffer untouched; only persistence skipped)
    }
}
```

- [ ] **Step 5: Gates + commit**

`cargo test -p wordcartel --lib state` green.
```bash
git add wordcartel/src/state.rs wordcartel/src/app.rs
git commit -m "feat(m5): cap session size (drop oversized scratch, bounded load)"
```

---

### Task 6: Swap / recovery bounded reads (`swap.rs`)

**Files:**
- Modify: `wordcartel/src/swap.rs` (`find_orphan_scratch_swap_in` ~:187, `assess` ~:241)
- Test: `swap.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::limits::MAX_OPEN_BYTES`.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn assess_over_cap_swap_opens_normally() {
    // Write an oversized swap at the doc's swap path; recovery must NOT slurp → OpenNormally.
    let p = scratch(); // a doc path
    let sp = swap_path(Some(&p)).unwrap();
    std::fs::write(&sp, "x".repeat(crate::limits::MAX_OPEN_BYTES as usize + 1)).unwrap();
    assert!(matches!(assess(Some(&p), None), RecoveryDecision::OpenNormally));
    let _ = std::fs::remove_file(&sp);
}
```
(If creating a 64 MiB file is too slow, use a sparse `set_len` write where the read path tolerates it,
or scope the test `#[ignore]` with a comment — but prefer a real over-cap file via `set_len` if the
reader uses `read_to_string` it will still try to read; a sparse file of NUL bytes reads fast.)

- [ ] **Step 2: A bounded-read helper (local to swap.rs)**

```rust
/// Read a swap file, refusing (None) if it exceeds the cap — never slurp unbounded.
fn read_swap_capped(path: &std::path::Path) -> Option<String> {
    let f = std::fs::File::open(path).ok()?;
    let cap = crate::limits::MAX_OPEN_BYTES;
    let mut s = String::new();
    std::io::Read::take(f, cap + 1).read_to_string(&mut s).ok()?;
    if s.len() as u64 > cap { return None; }
    Some(s)
}
```

- [ ] **Step 3: Route both read sites through it**

- `find_orphan_scratch_swap_in` (`swap.rs:187`): replace
  `let raw = match std::fs::read_to_string(entry.path()) { Ok(s) => s, Err(_) => continue };`
  with `let Some(raw) = read_swap_capped(&entry.path()) else { continue };`.
- `assess` (`swap.rs:241`): replace
  `let raw = match std::fs::read_to_string(&sp) { Ok(s) => s, Err(_) => return RecoveryDecision::OpenNormally };`
  with `let Some(raw) = read_swap_capped(&sp) else { return RecoveryDecision::OpenNormally };`.
(`use std::io::Read;` in scope.)

- [ ] **Step 4: Gates + commit**

`cargo test -p wordcartel --lib swap` green (existing recovery/orphan tests unaffected by the cap).
```bash
git add wordcartel/src/swap.rs
git commit -m "feat(m5): bound swap/recovery reads (over-cap swap treated as absent)"
```

---

### Task 7: Transform output cap (`transform.rs`)

**Files:**
- Modify: `wordcartel/src/transform.rs` (`TransformError`, `run_transform`)
- Test: `transform.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: `crate::limits::MAX_TRANSFORM_OUTPUT`.
- Produces: `TransformError::OutputTooLarge { limit: usize }`.

- [ ] **Step 1: Failing test**

A transform whose output would exceed the cap is refused. Since a real repar transform won't easily
produce 64 MiB from small input, test the guard directly by factoring the size-check into a tiny
checkable function, or assert on a crafted input near the boundary. Simplest: extract the check:
```rust
fn check_output_size(out: String) -> Result<String, TransformError> {
    if out.len() > crate::limits::MAX_TRANSFORM_OUTPUT {
        Err(TransformError::OutputTooLarge { limit: crate::limits::MAX_TRANSFORM_OUTPUT })
    } else { Ok(out) }
}

#[test]
fn transform_output_over_cap_refused() {
    let big = "x".repeat(crate::limits::MAX_TRANSFORM_OUTPUT + 1);
    assert!(matches!(check_output_size(big), Err(TransformError::OutputTooLarge { .. })));
    let ok = "small".to_string();
    assert!(check_output_size(ok).is_ok());
}
```

- [ ] **Step 2: Add the variant**

```rust
#[derive(Debug)]
pub enum TransformError { Repar(String), OutputTooLarge { limit: usize } }
```
Extend the `Display` match:
```rust
TransformError::OutputTooLarge { limit } => write!(f, "transform output too large (> {limit} bytes)"),
```

- [ ] **Step 3: Apply the check in `run_transform`**

```rust
pub fn run_transform(kind: TransformKind, input: &str, width: u32) -> Result<String, TransformError> {
    let mut opts = repar::Options::new().width(width);
    opts.apply_par_args([kind.verb()]).map_err(TransformError::from_repar)?;
    opts.apply_fixups("markdown").map_err(TransformError::from_repar)?;
    let out = opts.format(input).map_err(TransformError::from_repar)?;
    check_output_size(out)
}
```
(Known limitation, already in the spec: the formatter materializes `out` before the check; peak is
bounded by input ≈ doc size, not unbounded.)

- [ ] **Step 4: Gates + commit**

`cargo test -p wordcartel --lib transform` green.
```bash
git add wordcartel/src/transform.rs
git commit -m "feat(m5): refuse applying transform output over MAX_TRANSFORM_OUTPUT"
```

---

## Self-Review

**Spec coverage:** limits module + paste re-home (Task 1) ✔; filter raise (Task 1) ✔; undo byte budget + `current` fix + truncation subtraction + coalescing recompute + hint (Task 2) ✔; open bounded read + TooLarge (Task 3) ✔; search in-collector cap (Task 4) ✔; session scratch-guard + save drop + bounded load (Task 5) ✔; swap/recovery bounded reads (Task 6) ✔; transform output cap (Task 7) ✔. Out-of-scope (config, cumulative-doc ceiling, fuzz harness) untouched ✔.

**Type consistency:** `MAX_*` consts referenced identically across tasks; `all_matches(.., limit)` updated at all call sites (prod + 9 tests) in Task 4; `OpenError::TooLarge(String, u64)` and `TransformError::OutputTooLarge { limit }` consistent between definition and tests; `History.{bytes,last_evicted}` and `ChangeSet::stored_bytes` defined in Task 2 before use.

**Placeholder scan:** resolved against the real code (Codex plan review): filter harness is `run_filter(&spec, input, &CancelFlag::new())` (Task 1); buffer length is `TextBuffer::len()` not `len_bytes()` (Task 5); search ctor is `compile(needle, QueryMode::Literal, CaseMode::Sensitive)` not `Matcher::literal` (Task 4). The only remaining "confirm exact name" items are the `QueryMode`/`CaseMode` variant spellings and the `run_filter`/`CancelFlag` exact signature — both directly readable from the neighboring tests (search.rs:35, filter.rs:431).

**Ordering:** Task 1 establishes the consts every later task imports. Tasks 2-7 are independent and each leaves the crate compiling + green.

## Execution Handoff

Two execution options:
1. **Subagent-Driven (recommended)** — fresh subagent per task, two-stage review between tasks.
2. **Inline Execution** — batch with checkpoints.

Which approach?
