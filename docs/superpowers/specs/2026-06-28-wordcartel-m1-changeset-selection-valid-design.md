# M1 — ChangeSet/Selection Valid-by-Construction — Design

**Status:** Approved (brainstorm complete)
**Date:** 2026-06-28
**Parent:** Hardening campaign workstream **M1**, first item before Effort P
(`docs/superpowers/plans/2026-06-28-wordcartel-hardening-fuzz-proptest-plan.md`).
**Crate:** `wordcartel-core` (pure core) + a small shell migration in `wordcartel`.

## Goal

Make it **impossible to construct an inconsistent `ChangeSet` or `Selection`**, and make
`ChangeSet::apply` **fail fast on a precondition mismatch** (reject before mutating) —
closing the reachable-corruption hole the Codex blind-spot analysis flagged, without
speculative `Result` plumbing (that is deferred to the plugin boundary, M2 / Effort P).

## Background — the vulnerability (D0)

`ChangeSet` and `Selection` already have *validating constructors*, but their **public
fields** let callers bypass them and build inconsistent values:

- `ChangeSet { ops, len_before, len_after }` (change.rs:16–21) — fields public. The shell
  builds raw changesets at `commands.rs:125` (`build_range_replace`) and `commands.rs:157`
  (`build_multi_replace`), and core tests at `change.rs:430,437`. A `ChangeSet` whose
  `ops` do not sum to `len_before`/`len_after` drives `apply`'s unchecked `pos += n`
  (change.rs:83–95) into a `TextBuffer` char-boundary panic — *after partially mutating*.
- `apply` does **not** check `buf.len() == len_before` before running ops (change.rs:83).
- `Selection { ranges, primary }` (selection.rs:13–16) — fields public. A raw value with
  `primary >= ranges.len()` or empty `ranges` panics in `Selection::primary()`
  (selection.rs:50–56) in release (the bounds check is `debug_assert!` only).

The constructors (`ChangeSet::insert`/`delete`, `Selection::single`/`range`) always
produce valid values; the hole is the raw-field path.

## Decisions (from brainstorm)

1. **Scope = close-the-hole now, defer the `Result` boundary.** M1 makes the types
   valid-by-construction (private invariant-bearing fields + validated constructors) and
   adds `apply`'s precondition check. The `Result`-returning edit boundary lands in M2 /
   the plugin effort, where the untrusted caller (a plugin Transaction) actually appears.
   The shell is already valid-by-construction (`build_multi_replace` takes `doc_len`;
   positions are pre-clamped by `nav`), so no `Result` plumbing through editor commands.
2. **`apply` precondition = release-enforced `assert!(buf.len() == len_before)`** at entry.
   This guarantees "reject before mutating" for a **length mismatch** — fail fast, no
   partial edit, clear message; matches `TextBuffer`'s release-assert style (buffer.rs:30–63);
   one O(1) `len` compare, negligible on the hot path; the shell never trips it. (Codex
   confirmed it never false-positives on any current `apply` path — commit/undo/redo/
   coalescing all satisfy it.) **It does NOT cover op-boundary errors:** a valid-sum
   changeset with an op on a non-char-boundary would mutate earlier ops then panic in the
   buffer (a partial edit). Boundary safety stays on `TextBuffer`'s existing apply-time
   asserts; the full *no-partial-mutation op-boundary preflight* is **deferred to M2**
   (where untrusted plugin Transactions arrive). M1 claims only length-mismatch
   reject-before-mutating.
3. **Privatize only invariant-bearing fields.** `ChangeSet.{ops, len_before, len_after}`
   and `Selection.{ranges, primary}` go private. **`Range.{anchor, head}` stay public** —
   plain byte offsets with no cross-field invariant, read pervasively across `nav`/
   `commands`; privatizing them is gratuitous ripple for zero safety gain.
4. **`insert`/`delete` release-`assert!` (drop the silent clamp) (Codex Important 2).**
   D0 forbids clamping *edits* (clamp is for nav/UI recovery only). Today `insert`/`delete`
   `debug_assert` then **silently clamp** an out-of-range offset in release (change.rs:38–39,
   58–68) → a *silent wrong edit* (a clamped `delete` removes unintended bytes). Make them
   **release-`assert!`** their offset validity instead — uniform with `from_ops`/`apply`.
   Clamping stays the caller's job (`nav::clamp_snap`, already done by the shell), so this
   never trips for the shell (all shell offsets are pre-clamped). `delete` keeps its
   **reversed-range normalization** (interpreting `head..anchor` is not corruption) but
   asserts on **out-of-bounds** (`end > doc_len`).

## Components

### `ChangeSet` encapsulation (`wordcartel-core/src/change.rs`)

- Make `ops`, `len_before`, `len_after` **private**.
- **Harden `insert(at, text, doc_len)` / `delete(range, doc_len)` (decision 4):** replace
  the silent release-clamp with a **release-`assert!`** on offset validity — `insert`
  asserts `at <= doc_len`; `delete` asserts `end <= doc_len` (after normalizing a reversed
  `range`). They still produce the same valid ops for in-range input; the only change is
  an out-of-range offset now fails fast instead of silently clamping. The shell is
  unaffected (offsets pre-clamped by `nav`).
- **Add `ChangeSet::from_ops(ops: Vec<Op>, len_before: usize) -> ChangeSet`:**
  - Computes `len_after` from the ops (`sum(Retain) + sum(Insert byte len)`), so the
    caller cannot pass an inconsistent `len_after`.
  - **Asserts** (release-enforced) the consumption invariant: `sum(Retain) + sum(Delete)
    == len_before`. A violation is a trusted-caller bug → fail fast (consistent with the
    no-`Result` decision; the future plugin path validates + returns `Result` upstream,
    never calling `from_ops` directly with unchecked ops).
  - **NOT validated by `from_ops`:** UTF-8 op-boundary correctness — `from_ops` has no
    document, so it cannot check char boundaries. That safety is unchanged: it stays
    enforced by `TextBuffer`'s existing release char-boundary `assert!`s (buffer.rs:30–63)
    during `apply`. `from_ops` validates *structure* (op sums vs `len_before`); the buffer
    validates *boundaries* at apply time.
- **Add read accessors** — Codex's grep shows the only external read after privatization
  is `len_after` (selection.rs:135), so add **`len_after(&self) -> usize`**; add
  `ops()`/`len_before()` ONLY if the compiler shows a further external reader (core-internal
  readers like `map_pos`/`invert` access private fields directly).
- **`apply`:** add release-enforced `assert!(buf.len() == self.len_before, ...)` as the
  first statement, before the op loop (length-mismatch reject-before-mutating; see
  decision 2 for the boundary caveat).

### `Selection` encapsulation (`wordcartel-core/src/selection.rs`)

- Make `Selection.{ranges, primary}` **private**. Invariant: `!ranges.is_empty() &&
  primary < ranges.len()`.
- **Keep** `single(pos)` / `range(anchor, head)` (both produce one range, `primary = 0`
  — invariant holds). These are sufficient: Codex's grep found **no multi-range
  construction and no `primary != 0`** anywhere in the codebase. So **`from_ranges` and a
  `ranges()` accessor are YAGNI and are NOT added** (defer until a real multi-range caller
  exists — multi-cursor, post-1.0). The raw-`Selection`-literal sites migrate to `range`.
- `primary()` keeps its `debug_assert!` (now guaranteed by construction → belt-and-
  suspenders dev check; it can no longer fire from a public path).
- **`Range.{anchor, head}` unchanged (public).**

### Migration (shell + core tests)

- `commands.rs:125` (`build_range_replace`) and `commands.rs:157` (`build_multi_replace`):
  replace the raw `ChangeSet { ops, len_before: doc_len, len_after }` with
  `ChangeSet::from_ops(ops, doc_len)`. `from_ops` recomputes `len_after` from the ops —
  it must equal the hand-computed `len_after` these functions produce today (verify in a
  test; they consume the whole doc so `sum(Retain)+sum(Delete) == doc_len`).
- Core tests building raw `ChangeSet` (change.rs:307,430,437 and any others the compiler
  flags) → `from_ops` / `insert` / `delete`.
- **Convert the two release-only tests that lock in the OLD clamp behavior**
  (change.rs:243,256) to `#[should_panic]` (the new `insert`/`delete` assert on
  out-of-range offsets — decision 4).
- Migrate raw `Selection { ranges, primary }` sites to `Selection::range(anchor, head)`:
  the live site at `commands.rs:398` and the test sites Codex found
  (commands.rs:952,1112,1142,1163,1184,1247) — and any others the compiler flags.
- Add the `len_after()` accessor at its read site (selection.rs:135); add others only if
  the compiler flags them.

## Data flow (unchanged for the shell)

`build_multi_replace(edits, doc_len)` → `from_ops(ops, doc_len)` (validates, computes
`len_after`) → wrapped in a `Transaction` → `editor.apply` → `ChangeSet::apply(buf)`
(asserts `buf.len() == len_before`, then runs ops). For shell-built changesets the assert
always holds, so behavior is identical to today; the difference is only that inconsistent
values are now unconstructable and a mismatched `apply` fails fast.

## Error handling

- Invalid `ChangeSet`/`Selection` construction → **panic** (release assert) at the
  constructor. This is fail-fast for *trusted* callers (shell, core); the only caller is
  code that builds consistent values, so the panic is a bug-detector, never a runtime
  user path.
- Mismatched `apply` (`buf.len() != len_before`) → **panic** at `apply` entry, before any
  mutation. Cannot occur from the shell (valid by construction).
- No `Result`, no clamp, no silent corruption on the edit path. Graceful `Result`-based
  rejection for *untrusted* (plugin) Transactions is M2 / Effort P.

## Testing strategy

**T6 — `ChangeSet` validity:**
- `from_ops` computes `len_after` correctly for valid ops (unit + a proptest over random
  valid op sequences: `sum(Retain)+sum(Delete) == len_before`, `sum(Retain)+sum(Insert)
  == len_after`).
- `from_ops` panics (`#[should_panic]`) when `sum(Retain)+sum(Delete) != len_before`.
- `apply` panics (`#[should_panic]`) when applied to a buffer whose `len() != len_before`
  (the length-mismatch precondition — NOT a boundary test).
- `insert` panics (`#[should_panic]`) on `at > doc_len`; `delete` panics on an
  out-of-bounds `end > doc_len`; `delete` still normalizes a reversed in-bounds range
  (decision 4). The two former clamp tests (change.rs:243,256) become these.
- `build_multi_replace`/`build_range_replace` via `from_ops` produce the same `(ChangeSet,
  Edit)` as before (regression — compare against the existing
  `build_range_replace_yields_changeset_and_matching_edit` test at commands.rs:1261).

**T5 — `Selection` validity:**
- `single`/`range`/`from_ranges` always yield `!ranges.is_empty() && primary <
  ranges.len()`; `primary()` returns the expected range and never panics.
- `from_ranges` panics (`#[should_panic]`) on `primary >= ranges.len()` and on empty
  `ranges`.

**Regression:** the full `wordcartel-core` + `wordcartel` suites stay green; the
block_tree oracle + undo round-trip + all existing change/selection unit tests pass
unchanged.

## Out of scope (deferred)

- The `Result`-returning **edit boundary** for untrusted Transactions → **M2** (boundary
  harness) / **Effort P** (plugin `submit_transaction` validator).
- The **no-partial-mutation op-boundary preflight** in `apply` (validate every op's
  char-boundary against the buffer *before* the mutation loop) → **M2** (it matters once
  untrusted ops can reach `apply`; the shell never builds boundary-invalid ops). M1's
  `apply` assert covers length mismatch only.
- `Selection::from_ranges` + `ranges()` accessor (no multi-range caller exists yet) →
  whenever multi-cursor lands.
- T1–T4 core property tests + F1/F2 fuzz targets → rest of **M7**.
- `insert`/`delete` behavior change (they keep debug_assert + release-clamp; that is a
  *defined, non-corrupting* edit, not the raw-field hole).
- All other M-workstreams (M2–M5).

## New code surface (checklist for the plan)

- `change.rs`: 3 fields → private; `ChangeSet::from_ops(ops, len_before)` (validating);
  `len_after()` accessor (+ `ops()`/`len_before()` only if compiler-flagged); `apply`
  entry length-assert; **`insert`/`delete` clamp → release-assert** (decision 4).
- `selection.rs`: 2 fields → private (NO `from_ranges`/`ranges()` — YAGNI).
- `commands.rs`: `build_multi_replace`/`build_range_replace` → `from_ops`; raw `Selection`
  sites (commands.rs:398 + flagged test sites) → `Selection::range`.
- Core tests: raw `ChangeSet` → constructors; the 2 clamp tests (change.rs:243,256) →
  `#[should_panic]`.
- Tests: T5 + T6 as above (incl. the new insert/delete assert tests).
