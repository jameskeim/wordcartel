# Clippy-debt cleanup + house-style em-dash + durable clippy gate — design

**Status:** approved design (pre-spec-review)
**Date:** 2026-06-30
**Effort:** clippy-debt cleanup (pre-Effort-P housekeeping; user-approved 2026-06-29)

## Context

The repo carries pre-existing `clippy::all` warnings, so a clean workspace
`cargo clippy --all-targets -- -D warnings` was NEVER a real gate — CLAUDE.md
explicitly carves it out ("the codebase carries pre-existing clippy debt (tracked
as its own pre-Effort-P cleanup)"). This effort clears that debt to **zero**,
then locks it in with a hard gate so it cannot silently re-accumulate. It also
closes one house-style item explicitly deferred here from M4-rest (a `--` in a
user-facing string that should be an em-dash).

**Nothing here is user-facing.** Every clippy lint is about the shape of the Rust
source, not what the compiled program does; the fixes are behavior-preserving
refactors. The correctness guarantee is therefore "the full test suite stays
green after every fix" (876 tests today) — no new tests are added because no new
behavior is introduced.

## Goals

- Zero `clippy::all` warnings across `wordcartel-core` (lib + tests) and
  `wordcartel` (lib + tests).
- A durable gate so debt cannot return: `cargo clippy --all-targets` clean-or-error.
- Close the deferred `--`→`—` house-style item (+ any siblings).
- No behavior change; no regression (existing suite stays green).

## Non-goals

- No `cargo fmt` (repo is hand-formatted dense house style; this is a hard rule).
- No `clippy::pedantic`/`nursery`/`restriction` — only the default `clippy::all`
  group (pedantic would surface hundreds of subjective style opinions).
- No broad house-style audit (import grouping, inline-struct conventions, …) —
  only the bounded `--`→`—` sweep.
- No new runtime behavior, no new features, no unrelated refactoring.

## The lint inventory (current, on merged main @ 1f40f46)

Counts: `wordcartel-core` lib 15 (+2 test-only), `wordcartel` lib 18 (+9
test-only) — ~30–35 unique across the workspace.

### Mechanical set (~20) — idiomatic rewrites, zero behavior change
`needless_range_loop`, `unnecessary_map_or`, `filter_next` (→ `rfind`),
`redundant_pattern_matching`, `redundant_closure`, `needless_borrow`,
`map_identity`, `len_zero`, `collapsible_if`, `manual_div_ceil`,
`manual_range_contains`, `manual_clamp`, `unnecessary_cast`,
`unnecessary_unwrap`, `int_plus_one`, `option_map_unit_fn`,
`field_reassign_with_default`, `vec_init_then_push`, `collapsible_str_replace`,
`bool_assert_comparison` (tests).

### Semantic set (~8) — fixed PROPERLY (Q1 → B, no blanket allows)
- `ptr_arg` (×2): change `&Vec<T>` params to `&[T]` (callers unaffected — `&Vec`
  coerces to `&[]`).
- `type_complexity` (×1): extract a `type` alias for the complex type.
- `new_without_default` (×1): add a `Default` impl delegating to `new()`.
- `manual_checked_ops` (×1): use the checked operation — **preserve exact
  overflow/None semantics** (see Correctness watch).
- `inherent_to_string` (×2, incl. `TextBuffer::to_string`): implement
  `std::fmt::Display` instead of an inherent `to_string(&self) -> String`;
  `.to_string()` continues to work via the blanket `ToString` impl, so call sites
  are unaffected.

## The two judgment calls (STAGED FOR THE CODEX SPEC REVIEW)

Per Q1, "fix everything properly" is granted TWO explicit freedoms. The spec
records the decision framework; the **implementation must document the actual
per-site choice + reasoning in the code (a comment) and the task report**, and
the **Codex spec/plan review is asked to adjudicate whether each choice is the
right call** (fix-properly vs justified-allow vs rename).

### (a) `should_implement_trait` — `from_str` (×3: `wordcartel-core` `TextBuffer::from_str`, + 2 in shell)
Clippy flags an inherent method named `from_str` as confusable with
`std::str::FromStr::from_str`. The proper fix depends on fallibility:
- If the method is **infallible** (`fn from_str(&str) -> Self`, no `Result`), it
  CANNOT cleanly implement std `FromStr` (which requires `type Err` and returns
  `Result<Self, Err>`). Options, decided per-site by readability:
  1. **Rename** the inherent method (e.g. `from_str` → `from_text` / `parse` /
     `new`) and migrate call sites — zero user impact, some churn. Preferred when
     the new name reads clearly.
  2. **Justified `#[allow(clippy::should_implement_trait)]`** with a one-line
     rationale — when a rename reads worse than the lint and an `FromStr` impl is
     genuinely inapplicable.
- If a method is **fallible and FromStr-shaped**, implement `FromStr` properly.

The implementer picks per site and records the reasoning; Codex reviews the pick.
(`TextBuffer::from_str` is used widely — confirm its real signature/fallibility
in source before choosing.)

### (b) `too_many_arguments` (×1, shell)
- Refactor to a **struct-of-args** IF it genuinely improves clarity.
- Else a **`#[allow(clippy::too_many_arguments)]`** with a one-line rationale —
  a deliberately wide signature is sometimes clearer than an args struct.

The implementer picks and records the reasoning; Codex reviews the pick.

## House-style `--`→`—` sweep (Q4)

- Fix `wordcartel/src/app.rs` — the user-facing status string
  `"system clipboard unavailable -- copy/paste work in-editor; using OSC 52 for
  terminal sync"` (currently ~app.rs:797, moved by prior edits) — `--` → `—`.
- Grep the two crates for any OTHER `--` in user-facing strings or prose comments
  that the em-dash convention covers, and fix those too.
- If any test asserts one of these exact strings, update the assertion.
- Bounded strictly to `--`→`—`; NOT a broader style audit. (Clippy does not flag
  this — it is a CLAUDE.md house-style rule, separate from the lint work.)

## The durable gate

- Root `Cargo.toml`: add `[workspace.lints.clippy]` with `all = "deny"`. Each
  member crate opts in with `[lints]` `workspace = true`. Confirm the exact TOML
  shape against the real workspace layout during the plan.
- Item-local `#[allow(...)]` from the two judgment calls override the `deny` at
  the item, as intended.
- Because `clippy::all` lints are emitted only by `cargo clippy` (not `rustc`),
  `cargo build` / `cargo test` are UNAFFECTED — only `cargo clippy` enforces.
- **CLAUDE.md flip:** replace the "Do NOT gate on a clean
  `cargo clippy --all-targets -- -D warnings` over the whole workspace — the
  codebase carries pre-existing clippy debt" paragraph with a rule that
  workspace `cargo clippy --all-targets` clean IS a merge gate. This is a change
  to the project's own rules — the Codex review is asked to confirm it.

## Verification / testing

- **No new tests.** These are behavior-preserving refactors; correctness = the
  existing suite stays green. Run the covering tests at each task boundary and
  the full `cargo test` at the final gate (baseline 876 green).
- **Clippy clean:** `cargo clippy -p wordcartel-core -p wordcartel --all-targets`
  produces zero warnings at the end; after the gate lands it ERRORS on any
  regression (validating the `deny`).
- **No `cargo fmt`**, no reflow of untouched code.

## Correctness watch (the only real risk — a fix that silently changes behavior)

- `needless_range_loop`: confirm the loop index is used SOLELY for indexing the
  one collection before rewriting to an iterator; if the index is also used for
  arithmetic or a second collection, keep it (or `#[allow]` with reason).
- `manual_checked_ops` / `int_plus_one`: preserve exact semantics — a rewrite must
  not turn an overflow-panic into a silent `None`/wrap, or change a boundary
  (`a + 1 <= b` vs `a < b`) in a way that shifts an off-by-one.
- `field_reassign_with_default` / `vec_init_then_push`: ensure the rewritten
  literal preserves the exact initial values/order.
- `inherent_to_string` → `Display`: the `Display` output must be byte-identical to
  the old inherent `to_string`.

## Decomposition sketch (finalized in the plan; gate lands LAST on a clean tree)

1. `wordcartel-core` lib lints.
2. `wordcartel-core` test lints.
3. `wordcartel` lib lints (incl. the two judgment calls — highest scrutiny).
4. `wordcartel` test lints.
5. House-style `--`→`—` sweep.
6. Enable the gate (`Cargo.toml` lints) + CLAUDE.md flip — verified by clean
   `cargo clippy --all-targets` + green suite.

## Plan-confirms (resolve during the implementation plan, against real source)

1. The exact `TextBuffer::from_str` signature/fallibility (and the 2 shell
   `from_str` sites) to decide rename-vs-allow-vs-FromStr per site.
2. The `too_many_arguments` function + its call sites to decide struct-vs-allow.
3. The exact `Cargo.toml` workspace layout + `[workspace.lints]`/`[lints]` TOML.
4. The precise CLAUDE.md paragraph to replace + its replacement text.
5. Each `needless_range_loop` / `manual_checked_ops` / `int_plus_one` site's real
   surrounding code, to confirm the rewrite is semantics-preserving.
6. The full grep result for `--` in user-facing strings / prose comments (the
   sweep set) + any test asserting those exact strings.
