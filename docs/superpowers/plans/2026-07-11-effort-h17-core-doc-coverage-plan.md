# Effort H17 — implementation plan: `wordcartel-core` doc-coverage sweep + `missing_docs` gate

**Spec:** `docs/superpowers/specs/2026-07-11-effort-h17-core-doc-coverage-design.md` (approved
2026-07-11, Scope A, convention (a) for the fuzzing-cfg gate).
**Branch:** `effort-h17-core-doc-coverage`.
**Shape:** enable the gate first (the `missing_docs` lint becomes the red oracle), then document
file-by-file until each file's warning count is zero; a final task confirms the whole crate is green.
Subagent-driven, cheapest model per implementer (doc-writing is mechanical once the policy is fixed),
per-task doc-quality reviewer. One Codex pre-merge gate. No Fable.

---

## Global constraints (bind EVERY task — copy into each implementer/reviewer dispatch)

1. **Doc comments only — no logic, no signature, no visibility changes.** The ONLY non-doc edit in the
   whole effort is Task 1's single crate-level attribute. Never turn `pub` into `pub(crate)` (even if an
   item looks like it shouldn't be public — document it, and note the observation in your report; a
   visibility change is a separate effort). Never reorder or edit code.
2. **House style** (CLAUDE.md → "Docs"): `///` outer doc on each public item; `//!` inner doc for a
   module. Document params, returns, and errors for functions. **Substance over stubs** — a real phrase,
   never `/// The foo.` that just echoes the name. Fields/enum variants get a short accurate phrase;
   functions get purpose + params/returns/errors. `—` (em-dash) in prose, never `--`. No emoji.
3. **`# Examples` only on non-obvious public *functions/methods*** — NOT on fields, enum variants, type
   aliases, or trivial accessors/getters. Any `# Examples` block is a **runnable doctest**: it must
   compile and pass (`cargo test -p wordcartel-core --doc`). Keep examples minimal, correct, and use the
   crate's real public API (add the `use` lines the doctest needs). If an example would need private
   internals, the item is not a good example target — skip it.
4. **Anchor by symbol NAME, not line number.** Adding docs shifts every line below; locate items by
   name/structure (grep / `documentSymbol`), never by a recorded `:NNN`.
5. **Tooling: `cargo` + `grep` are ground truth, not the editor.** For "is this documented / does this
   compile" questions, trust `cargo build`/`cargo test` output, NOT a rust-analyzer "unused"/"undefined"
   hint (subagent edits are the most stale in an analyzer's view).
6. **Per-file green check** (the task's definition of done): rustc prints the `warning: missing
   documentation …` message and the `--> …/src/X.rs:LN` path on **separate lines**, so a naive
   `grep 'missing documentation' | grep src/X` matches nothing and gives a FALSE clean. Use a
   **context-aware count** that pairs each `missing documentation` message with its following `-->`
   location line, then filters to the file:
   ```
   cargo build -p wordcartel-core 2>&1 \
     | awk '/missing documentation/{f=1} f&&/-->/{print $2; f=0}' | grep -c 'src/X\.rs'          # -> 0
   cargo test  -p wordcartel-core --no-run 2>&1 \
     | awk '/missing documentation/{f=1} f&&/-->/{print $2; f=0}' | grep -c 'src/X\.rs'          # -> 0
   cargo test  -p wordcartel-core --doc 2>&1 | tail -3                                             # doctests pass
   ```
   (The `awk` pairs message→location so only `missing_docs` spans count, robust even if an unrelated
   diagnostic appears; `\.rs` anchors the extension so `src/change.rs` can't match `src/change_x.rs`.
   The `--no-run` line matters only for `test_support.rs`, but run both — cheap. Verified against this
   crate's real output: it reproduces the Phase-0 worklist counts exactly.)
7. **Commit per task** with the project trailers (Co-Authored-By + Claude-Session). Message form:
   `docs(h17): document <file(s)> public items`.

---

## Task 1 — Enable the gate (the red oracle)

**File:** `wordcartel-core/src/lib.rs`. Add `#![warn(missing_docs)]` to the existing crate-level
attribute block, immediately after the H7 clippy-cast `#![deny(...)]` line (currently line ~9), with a
one-line rationale comment in the house style, e.g.:

```rust
// H17: public-API doc-coverage gate. `warn` (not `deny`) — teeth come from the repo's
// warning-free-build merge gate; deny would only add local-iteration friction.
#![warn(missing_docs)]
```

**Verify (this task's "test"):** the crate still compiles, and the oracle is now live:
```
cargo build -p wordcartel-core                                              # compiles (warn ≠ error)
cargo build -p wordcartel-core 2>&1 | grep -c 'missing documentation'       # == 234
cargo test  -p wordcartel-core --no-run 2>&1 | grep -c 'missing documentation'  # == 237
```
Do NOT document anything in this task. Commit just the attribute. (After this, the branch's
warning-free gate is intentionally RED until Task 9 — expected on a branch.)

**Reviewer focus:** attribute placed in the right block; rationale present; no other change; counts match.

---

## Tasks 2–8 — Document, file by file (each drives its file's warning count to 0)

Each task: document every public item the oracle flags in the named file(s), to the Global-constraints
standard, until the per-file green check (constraint 6) is clean. Cheapest model. Counts are from the
Phase-0 worklist (spec); treat the live `grep` as authoritative if a count drifted.

- **Task 2 — `theme.rs` (84).** The heaviest, but largely terse: chrome-face fields and exhaustive
  `SemanticElement`/face enum variants. Describe what each face/field *is*; do NOT invent semantics or
  add `# Examples` to variants/fields. Cross-reference the theme model where a one-liner needs context.
- **Task 3 — `style.rs` (32).**
- **Task 4 — `history.rs` (28).** Undo/redo public surface — functions here likely warrant `# Examples`
  on the non-obvious ones (the public API is `commit`/`undo`/`redo`/`commit_coalescing` — there is no
  public `push`); document error/return conditions.
- **Task 5 — `block_tree.rs` (22) + its module doc + trait items.** Add the missing `//!` module doc at
  the top of `block_tree.rs` (this also clears the sole `lib.rs`=1 hit — the undocumented
  `pub mod block_tree;`). Document the `TextSource` trait's associated items (`len`, `is_empty`, `slice`) and all
  other flagged public items. Note `incremental_equals_full` is `cfg(any(test, fuzzing))` public — it is
  flagged and must be documented like any other.
- **Task 6 — `selection.rs` (12) + `change.rs` (9) + `register.rs` (7).** Core edit-data types.
  `ChangeSet` is the validated-constructor API — document its constructors' invariants/errors; good
  `# Examples` candidates.
- **Task 7 — `diagnostics.rs` (12) + `search.rs` (11).** `diagnostics.rs` is the provider-neutral
  diagnostic vocabulary kept after Effort A removed the Harper backend — document it as pure data.
- **Task 8 — `buffer.rs` (10) + `layout.rs` (5) + `test_support.rs` (3) + `outline.rs` (1).**
  `test_support.rs`: the 3 `EditOp` fields (`at`/`del`/`ins`) — one accurate phrase each (these are the
  `cargo test --no-run`-only items). `outline.rs`: the single flagged item.

**Reviewer focus (every task):** docs are *substantive* (spot for stubs echoing the name); params/
returns/errors present on functions; `# Examples` only where warranted and they compile; **no visibility
change, no logic change**; the file's green check is clean.

---

## Task 9 — Whole-crate green (the gate)

No new docs unless the check surfaces a straggler (then document it in place). Confirm the full intrinsic
gate:
```
cargo build -p wordcartel-core 2>&1 | grep -c 'missing documentation'         # == 0
cargo test  -p wordcartel-core --no-run 2>&1 | grep -c 'missing documentation'# == 0
cargo test  -p wordcartel-core                                                # green (incl. doctests)
cargo clippy --workspace --all-targets                                        # clean
cargo test  -p wordcartel                                                     # shell suite still green
```
If any stragglers appear (e.g. a re-export or macro-generated public item the per-file worklist didn't
name), document them and note them in the report. Commit any stragglers; otherwise this task is a
verification checkpoint. Quote the smoke-suite summary per the pre-merge report contract
(`scripts/smoke/run.sh`).

---

## Final gate (controller)

One **Codex pre-merge GO/NO-GO**: spot-check doc *accuracy* against the code, confirm **no visibility was
narrowed** to silence the lint, confirm `# Examples` are correct, confirm the attribute + convention
match the spec. **No Fable whole-branch** — the compiler enforces completeness; there is no cross-task
invariant to synthesize. On GO, merge `--no-ff` to `main`; verify the gate green on the merge; delete the
branch. Push only when asked.

## Command-surface-contract conformance

**N/A — does not touch the command surface.** No command, option, palette entry, menu item, or keybinding
hint is added, removed, or changed; the effort adds only doc comments and one crate-level lint attribute.
