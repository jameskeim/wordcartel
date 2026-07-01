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

**Counts below are INDICATIVE, not authoritative.** They drift as the tree
changes. The PLAN must enumerate every warning site from a FRESH
`cargo clippy --all-targets` run (per crate, lib + tests) and fix exactly that
set. Two indicative counts in the first draft were already stale (Codex spec
review): there are **zero** shell inherent `from_str` *definitions* (the shell
`from_str` hits are *calls* to `TextBuffer::from_str`/`Rope::from_str`/
`toml::from_str`), and there is exactly **one** inherent `to_string`
(`wordcartel-core/src/buffer.rs:100`), not two.

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
- `inherent_to_string` (×1: `TextBuffer::to_string`, `buffer.rs:100`, which
  delegates to `self.rope.to_string()`): implement `std::fmt::Display` instead of
  the inherent `to_string(&self) -> String`; `.to_string()` continues to work via
  the blanket `ToString` impl, so call sites are unaffected. Confirm no other
  inherent `to_string` exists from the fresh clippy run.

## The two judgment calls (STAGED FOR THE CODEX SPEC REVIEW)

Per Q1, "fix everything properly" is granted TWO explicit freedoms. The spec
records the decision framework; the **implementation must document the actual
per-site choice + reasoning in the code (a comment) and the task report**, and
the **Codex spec/plan review is asked to adjudicate whether each choice is the
right call** (fix-properly vs justified-allow vs rename).

### (a) `should_implement_trait` — every inherent method clippy flags as confusable with a std trait method
This lint fires on an inherent method whose NAME matches a std trait method
(`from_str`→`FromStr`, `default`→`Default`, `next`→`Iterator`, `add`→`Add`, …).
The confirmed sites (identify the exact set from the fresh clippy run — 1 core
site plus the shell sites clippy reports on OTHER inherent methods; the shell has
no inherent `from_str` definition):
- **`wordcartel-core` `TextBuffer::from_str`** (`buffer.rs:11`, `fn from_str(&str)
  -> Self` — **infallible**, verified): cannot cleanly implement std `FromStr`
  (which needs `type Err` + returns `Result<Self, Err>`). Options, per-site by
  readability: (1) **Rename** (`from_str` → `from_text`/`parse_str`/`new`) and
  migrate call sites; (2) **justified `#[allow(clippy::should_implement_trait)]`**
  with a one-line rationale. **NOTE: rename touches ~61 call sites** across core,
  shell, tests, AND the detached fuzz crate (which must be updated separately) —
  so `#[allow]` is a strong candidate here; the implementer weighs churn vs
  clarity and records the reasoning.
- **The shell site(s)** — at least **1 is source-confirmed: `SearchState::next`**
  (`search_overlay.rs:129`, confusable with `Iterator::next`); any additional
  shell `should_implement_trait` sites are identified from the fresh clippy run
  (some may be in test code). For each: if the inherent method could CLEANLY
  implement the matching std trait (e.g. `Iterator` for `next`), do so; else
  rename or justified `#[allow]`.

The implementer picks per site and records the reasoning; **Codex reviews each
pick** (the user explicitly asked for this adjudication).

### (b) `too_many_arguments` (×1, shell — `apply_filter_done`, `app.rs:274`, 8 args)
`apply_filter_done(editor, buffer_id, version, range, cursor, disposition,
outcome, clock)` is called only from the two `Msg::FilterDone` match arms
(`app.rs` ~1438, ~1731). A struct-of-args would largely duplicate the existing
`Msg::FilterDone` fields (Codex: `#[allow]` is the defensible call).
- Refactor to a **struct-of-args** IF it genuinely improves clarity.
- Else a **`#[allow(clippy::too_many_arguments)]`** with a one-line rationale.

The implementer picks and records the reasoning; Codex reviews the pick.

## House-style `--`→`—` sweep (Q4)

- Fix `wordcartel/src/app.rs` — the user-facing status string
  `"system clipboard unavailable -- copy/paste work in-editor; using OSC 52 for
  terminal sync"` (currently ~app.rs:797, moved by prior edits) — `--` → `—`.
- Grep the two crates for any OTHER `--` in user-facing strings or prose comments
  that the em-dash convention covers, and fix those too — **with editorial
  judgment**. The em-dash rule applies ONLY to prose. Do NOT touch: markdown list
  syntax / test-fixture markdown (`-- ` bullets, `---` rules), CLI flag arguments
  (`--config`, `--no-config`), shell-command examples, `// ----` separator-rule
  comments, or any `--` that is not a real prose em-dash. `app.rs:797` is the one
  CONFIRMED target; any additional site requires a clear prose-em-dash reading.
- If any test asserts one of these exact strings, update the assertion.
- Bounded strictly to `--`→`—`; NOT a broader style audit. (Clippy does not flag
  this — it is a CLAUDE.md house-style rule, separate from the lint work.)

## The durable gate

- Root `Cargo.toml`: add `[workspace.lints.clippy]` with `all = "deny"`. Each
  member crate (`wordcartel-core`, `wordcartel` — the only two members; the fuzz
  crate is a detached sub-workspace and stays out of scope) opts in with `[lints]`
  `workspace = true`.
- **Migrate the existing lint table.** `wordcartel-core/Cargo.toml` ALREADY has a
  `[lints.rust]` section (M7's `unexpected_cfgs` = `{ level = "warn", check-cfg =
  ['cfg(fuzzing)'] }`). A crate CANNOT set both `[lints] workspace = true` and its
  own local `[lints.*]` tables — so this entry must MOVE into
  `[workspace.lints.rust]` in the root `Cargo.toml` (then `wordcartel-core`
  inherits it via `workspace = true`). Applying `check-cfg = ['cfg(fuzzing)']`
  workspace-wide is harmless for the shell (it never uses `cfg(fuzzing)`; the flag
  only registers the name as known). Confirm the exact TOML against the real
  layout during the plan.
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

1. The exact set of `should_implement_trait` sites from a fresh clippy run: the
   confirmed core `TextBuffer::from_str` (infallible → rename-vs-allow, weighing
   the ~61-site rename churn) PLUS the shell sites' actual method names (1
   source-confirmed: `SearchState::next`; pin the rest from clippy) — decide
   implement-trait vs rename vs allow per site.
2. `apply_filter_done` (`app.rs:274`) struct-vs-allow (Codex leans `#[allow]`).
3. The exact `Cargo.toml` workspace layout + `[workspace.lints]`/`[lints]` TOML,
   INCLUDING migrating `wordcartel-core`'s existing `[lints.rust] unexpected_cfgs`
   into `[workspace.lints.rust]` (can't mix `workspace = true` with local lints).
4. The precise CLAUDE.md paragraph to replace + its replacement text.
5. Each `needless_range_loop` / `manual_checked_ops` / `int_plus_one` site's real
   surrounding code, to confirm the rewrite is semantics-preserving.
6. The full grep result for `--` in user-facing strings / prose comments (the
   sweep set) + any test asserting those exact strings.
