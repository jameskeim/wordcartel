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
   only a `Transaction`; the boundary derives a **conservative** `block_tree::Edit`
   (`range: 0..len_before`, `new_len: len_after` → a full reparse) and uses a fixed
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
    /// `len_before == buf.len()` then walks every op, confirming each Delete/Insert
    /// byte position is a char boundary in `buf`. Returns the FIRST violation. All ops
    /// are checked before any caller mutates — the no-partial-mutation guarantee.
    pub fn validate_against(&self, buf: &TextBuffer) -> Result<(), EditError>;
}
```

- Lives in core because it needs the private `is_char_boundary` (buffer.rs:25) and the
  private `ops` — and it belongs next to `apply`. No new public field accessor is needed
  (it uses the private fields directly).
- `EditError` is the core type; the shell re-exports it.
- The op-position walk mirrors `apply`'s cursor walk: `Retain(n)` advances `pos`;
  `Delete(n)`/`Insert(_)` happen *at* `pos` — that `pos` must be a char boundary. (Insert
  text content is always valid UTF-8 — a `&str` — so only the insertion *position* needs
  checking, not the inserted bytes.)

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

Flow:
1. `let buf = &editor.active().document.buffer;` `txn.changes.validate_against(buf)?` —
   returns `Err(StaleLength|OpBoundary)` with zero mutation.
2. Snap the selection: if `txn.selection` is `Some(sel)`, map each `Range`'s `anchor`/`head`
   through `clamp_snap`-style logic into `[0, len_after]` on a char boundary; rebuild the
   `Transaction` with the snapped selection. (Reuse `nav::clamp_snap` or the same
   clamp+grapheme-snap primitive; the post-edit buffer is needed for boundary snapping —
   see "Selection snapping" below.)
3. Derive `let edit = block_tree::Edit { range: 0..len_before, new_len: len_after };`.
4. `editor.apply(snapped_txn, edit, EditKind::Other, clock);` → `Ok(())`.

**Selection snapping detail:** the selection is *post-edit* positions, so snapping to a
char boundary needs the *post-edit* text. Two valid implementations: (a) range-clamp the
selection to `[0, len_after]` *before* apply (cheap, guarantees in-bounds → `apply`'s
selection set is valid), then a boundary-snap pass *after* apply against the now-current
buffer; or (b) compute the post-edit length and clamp pre-apply, relying on
`apply`/downstream nav to keep the cursor on a boundary. Implementation picks the
simplest that guarantees the final selection is in-bounds and on a char boundary. Either
way, a bad selection is **never** a reject — it snaps.

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
snap selection → derive conservative Edit → `editor.apply` (trusted) → `Ok`. Internal
trusted edits bypass this entirely (continue calling `editor.apply` directly).

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

- `wordcartel-core/src/change.rs`: `EditError` enum; `ChangeSet::validate_against(&self,
  buf: &TextBuffer) -> Result<(), EditError>`; `EditError` exported from the crate.
- `wordcartel/src/transact.rs` (new): `submit_transaction(editor, txn, clock) ->
  Result<(), EditError>`; re-export `EditError`; the selection-snap + conservative-Edit
  logic. Declare `mod transact;`.
- Tests: `change.rs` unit tests for `validate_against`; `transact.rs` unit + proptest
  harness for `submit_transaction`.
