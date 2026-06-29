# M2 — Adversarial Edit-Submission Boundary — Design

**Status:** Approved (brainstorm complete)
**Date:** 2026-06-28
**Parent:** Hardening campaign workstream **M2**
(`docs/superpowers/plans/2026-06-28-wordcartel-hardening-fuzz-proptest-plan.md`).
**Crate:** `wordcartel-core` (validation) + `wordcartel` shell (the boundary + harness).

## Goal

Provide a validated, plugin-ready **edit-submission boundary** that accepts an
**untrusted** `Transaction`, validates it against the *live* buffer, and either applies
it (trusted path) or returns `Err` with **zero mutation** — never panics, never partially
edits. This is the seam Effort P's Lua `apply(Transaction)` will call, and it closes the
no-partial-mutation op-boundary gap M1 explicitly deferred. Plus the adversarial harness
proving the guarantee.

## Background

- M1 made `ChangeSet` valid-by-construction (private fields; `from_ops` asserts the op
  sums). So an untrusted caller **cannot** build a structurally-inconsistent `ChangeSet`.
  The remaining untrusted-input risks are **buffer-relative**, which `from_ops` cannot
  check because it has no document:
  - a Transaction built for a **stale document length** (M1's `apply` asserts
    `buf.len() == len_before` — a *panic*, not a graceful `Result`);
  - an op landing on a **non-char-boundary** (M1's `apply` asserts length only;
    a valid-sum changeset with a mid-char op position would mutate earlier ops, then
    panic deep in `TextBuffer` — a *partial edit*).
- The edit path has **no single choke-point**: `editor.apply(txn, edit, kind, clock)`
  (editor.rs:635 → Buffer::apply editor.rs:179) is called directly from ~40 internal
  sites. Those internal edits are **trusted** (built via `build_multi_replace` from the
  live `doc_len`, valid by construction) and keep calling `editor.apply` directly. M2
  does NOT funnel them — it adds a NEW, separate **untrusted** entry point.
- The async seam (`reduce`/`apply_job_result`, `InlineExecutor`, `TestClock`,
  `Msg::JobDone`/`FilterDone`/…) is already well-tested for stale/late results,
  buffer-close-mid-job, and quit-drain. M2 does not re-cover it.

## Decisions (from brainstorm)

1. **Build the boundary now** (not harness-only): `submit_transaction` is the plugin
   substrate, built + proven standalone ahead of Effort P. Its only caller until P is the
   harness — that is the point.
2. **Minimal plugin API + conservative reparse Edit.**
   `submit_transaction(editor, txn, clock) -> Result<(), EditError>`. The plugin supplies
   only a `Transaction`; the boundary derives a **conservative whole-document**
   `block_tree::Edit` (`range: 0..len_before`, `new_len: len_after`) — a whole-doc Edit
   through the existing **incremental** derive path (`incremental_update_rope`, NOT a
   direct `full_parse_rope` call; functionally a full reparse) — and uses a fixed
   `EditKind::Other`. Plugin edits are infrequent and off the per-keystroke hot path, so a
   full reparse is an acceptable cost; the incremental path stays for real typing. (A
   tight/incremental Edit derivation can be added later if plugin edits become frequent.)
3. **Hard-reject the mutation; snap the cursor.** The mutation-critical checks
   (length match, op boundaries) **reject → `Err`, zero mutation**. The Transaction's
   **selection** (cursor positions) is **clamp+snapped** into range — never a reject
   reason. This matches D0 (mutations fail-fast; cursor/nav positions snap, as
   restore/resume offsets already do via `nav::clamp_snap`). So `EditError` has two
   variants: `StaleLength`, `OpBoundary`.

## Components

### 1. Validation in core (`wordcartel-core/src/change.rs`)

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditError {
    StaleLength { expected: usize, actual: usize }, // len_before != buf.len()
    OpBoundary { pos: usize },                       // an op position is not a char boundary
}

impl ChangeSet {
    /// Validate this changeset against `buf` WITHOUT mutating. Checks
    /// `len_before == buf.len()` (→ `StaleLength`) then walks every op, confirming each
    /// op's byte boundaries are char boundaries in `buf` (→ `OpBoundary`). Returns the
    /// FIRST violation. All ops are checked before any caller mutates — the
    /// no-partial-mutation guarantee.
    pub fn validate_against(&self, buf: &TextBuffer) -> Result<(), EditError>;
}
```

- Lives in core because it needs `TextBuffer::is_char_boundary` and the private `ops`,
  and it belongs next to `apply`. **`buffer::is_char_boundary` (buffer.rs:25) is currently
  private-to-module — M2 makes it `pub(crate)`** so `change.rs` (same crate, different
  module) can call it. `TextBuffer::len()` already exists (buffer.rs:15). No new public
  field accessor is needed (same-module access to `ops`).
- `EditError` is the core type; the shell re-exports it.
- **The op-walk tracks an OLD-text cursor (positions are byte offsets into the
  current/before buffer) (Codex Critical):**
  - `Retain(n)` → advance `old_pos` by `n`.
  - `Delete(n)` → check **BOTH** `old_pos` and `old_pos + n` are char boundaries (M1's
    `TextBuffer::delete` asserts both endpoints, buffer.rs:38 — checking only the start
    would still let a delete that *ends* mid-multibyte-char panic after earlier ops
    mutated), then advance `old_pos` by `n`.
  - `Insert(_)` → check `old_pos` only (the inserted text is a valid-UTF-8 `&str`, so only
    the insertion *position* matters; `old_pos` does not advance — insert adds to the new
    text, not the old).
  On the first non-boundary position, return `Err(OpBoundary { pos })` with that byte
  offset.

### 2. The boundary (new module `wordcartel/src/transact.rs`)

```rust
pub use wordcartel_core::change::EditError;

/// Untrusted edit-submission boundary (Effort P's `apply(Transaction)` seam). Validates
/// `txn` against the active buffer; on `Err` returns immediately with NOTHING mutated.
/// On success: snaps the selection, derives a conservative full-doc reparse Edit, and
/// applies via the trusted `editor.apply`. Never panics, never partially edits.
pub fn submit_transaction(
    editor: &mut Editor,
    txn: Transaction,
    clock: &dyn Clock,
) -> Result<(), EditError>;
```

Flow (the snap is FULLY pre-apply — Codex Important):
1. `txn.changes.validate_against(&editor.active().document.buffer)?` — returns
   `Err(StaleLength|OpBoundary)` with **zero mutation**.
2. **Snap the selection against a CLONE, before touching the live buffer.** Clone the
   active buffer (O(1) ropey snapshot/clone), apply the *validated* `ChangeSet` to the
   clone (gives the post-edit text without mutating the live document), then snap the
   selection against the clone. Snapping must be **pre-apply** because `editor.apply`'s
   `commit_coalescing` records the selection in the undo revision — passing an *unsnapped*
   selection would store a bad cursor that a later **redo** restores and panics on
   (Codex). So: build the final `Transaction` with the *original* `ChangeSet` + the
   *snapped* selection, and only then call `editor.apply`.
3. Derive the conservative whole-doc Edit:
   `block_tree::Edit { range: 0..len_before, new_len: len_after }`.
4. `editor.apply(snapped_txn, edit, EditKind::Other, clock);` → `Ok(())` — the single,
   real mutation.

**Selection snapping detail:**
- **Single-range only (Codex Important).** Selections are single-range everywhere today
  (M1's Codex grep found no multi-range, no `primary != 0`), and M1 deliberately did NOT
  add `Selection::from_ranges`/`ranges()`. So M2 reads the one range via `sel.primary()`
  (`Range.anchor`/`.head` are public) and rebuilds via `Selection::range(snapped_anchor,
  snapped_head)`. No new `Selection` API. (Multi-range snapping waits for a multi-cursor
  effort that adds the accessors.)
- **Use a NEW buffer-only core helper, NOT `nav::clamp_snap` (Codex re-review).**
  `nav::clamp_snap(&Editor, off)` is **layout-coupled** — it calls `get_or_layout`
  (nav.rs:164) which depends on document blocks, view mode, caret line, text geometry, and
  theme glyph. It is NOT a buffer-only function and cannot be cleanly decomposed to take a
  `&TextBuffer`. M2 does not need *layout-aware* (visual cursor-stop) snapping for a
  plugin's programmatic cursor — it only needs the selection to be a **valid byte offset**
  (in `[0, len]`, on a char boundary) so nothing slices mid-char. That is a pure
  `TextBuffer` concern. So M2 adds **`TextBuffer::clamp_to_boundary(&self, off: usize) ->
  usize`** (in buffer.rs, where `is_char_boundary` lives): clamp `off` to `[0, len]`, then
  floor to a char boundary (`rope.char_to_byte(rope.byte_to_char(off))`). The snapped
  anchor/head are `clone.clamp_to_boundary(raw_anchor)` / `clone.clamp_to_boundary(
  raw_head)`. **`nav::clamp_snap` is left untouched.** (The editor re-applies any
  layout-aware visual snapping on the next nav/render anyway; the byte-boundary clamp is
  what guarantees safety.)
- A bad selection is **never** a reject — it snaps.

### 3. The adversarial harness (tests in `transact.rs`)

- **Unit tests:**
  - A valid `Transaction` (built via `from_ops` for the current `doc_len`) applies; buffer
    becomes the expected text; selection lands correctly.
  - `StaleLength`: a Transaction whose `ChangeSet` was built for a *different* `doc_len`
    → `Err(StaleLength{..})`, buffer **byte-identical** to before.
  - `OpBoundary`: a Transaction with an op position inside a multibyte char (e.g. `delete`
    at byte 1 of `"é…"`/`"中"`) → `Err(OpBoundary{pos})`, buffer **byte-identical**,
    **no panic**.
  - Selection snapping: a valid edit whose Transaction carries a selection past
    `len_after` (or on a mid-char position) → edit applies, cursor snapped into range on a
    boundary, **no panic**.
- **Property test (proptest) — the core M2 guarantee:** random unicode document ×
  randomly-built `Transaction` (a mix: valid changesets, changesets built for the wrong
  `len_before`, op positions placed mid-char, wild selections) through
  `submit_transaction`:
  - **NEVER panics.**
  - On any `Err`, the buffer is **byte-identical to before** (no partial mutation).
  - On `Ok`, the buffer length equals `len_after` and the selection is in `[0, len_after]`
    on a char boundary.

## Data flow

Untrusted caller (harness now; Lua plugin in P) → `submit_transaction(editor, txn, clock)`
→ `txn.changes.validate_against(active_buf)` → on `Err`: return (no mutation); on `Ok`:
snap selection against a *clone* (apply the validated ChangeSet to a buffer clone, snap
the single range with `clone.clamp_to_boundary`) → derive conservative whole-doc Edit → build the
final `Transaction` (original ChangeSet + snapped selection) → `editor.apply` (trusted,
the only live mutation) → `Ok`. Internal trusted edits bypass this entirely (continue
calling `editor.apply` directly).

## Error handling

- `StaleLength` / `OpBoundary` → `Err`, zero mutation, no panic. The caller (plugin)
  surfaces it (status / disables itself) — that wiring is Effort P; M2 returns the typed
  error.
- Bad selection → snapped, never an error.
- `submit_transaction` itself never panics for any input.

## Testing strategy

See Component 3. The proptest's two invariants — *never panics* and *Err ⟹ buffer
unchanged* — are the deliverable. Plus: all existing core + shell suites stay green
(`submit_transaction` is additive; it does not change `editor.apply` or any internal edit
path).

## Out of scope (deferred)

- **IO fault injection** (filesystem trait; disk-full/write-fail/fsync-fail) → **M3**.
- **Foreground merge-closure panic isolation** + other async-seam panic gaps → **M4**
  (async/panic hardening). The async result seam's *staleness* handling is already tested.
- **Wiring `submit_transaction` to a real caller** (Lua `apply`, error surfacing, the
  plugin permission/budget model) → **Effort P**. M2 builds + proves the boundary; P
  consumes it.
- **Tight/incremental Edit derivation** (conservative full-doc reparse is used) → later,
  only if plugin edits become frequent.

## New code surface (checklist for the plan)

- `wordcartel-core/src/buffer.rs`: make `is_char_boundary` **`pub(crate)`** (currently
  private-to-module) for `validate_against`; add **`pub fn clamp_to_boundary(&self, off:
  usize) -> usize`** (clamp to `[0, len]` + floor to char boundary) for the selection
  snap. `len()` already public. (`nav::clamp_snap` is NOT touched — it is layout-coupled.)
- `wordcartel-core/src/change.rs`: `EditError` enum (exported from the crate);
  `ChangeSet::validate_against(&self, buf: &TextBuffer) -> Result<(), EditError>` with the
  OLD-cursor op-walk (Delete checks BOTH endpoints).
- `wordcartel/src/transact.rs` (new): `submit_transaction(editor, txn, clock) ->
  Result<(), EditError>`; re-export `EditError`; the clone-snap-then-apply flow
  (single-range selection via `primary()` + `TextBuffer::clamp_to_boundary` +
  `Selection::range`) + conservative-Edit derivation. Declare `mod transact;`. (No
  `nav.rs` change.)
- Tests: `change.rs` unit tests for `validate_against` (incl. delete-end-mid-char →
  `OpBoundary`); `transact.rs` unit + proptest harness for `submit_transaction`.
