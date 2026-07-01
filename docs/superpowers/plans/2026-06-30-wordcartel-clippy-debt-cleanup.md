# Clippy-debt cleanup + em-dash sweep + durable gate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clear every `clippy::all` warning across `wordcartel-core` + `wordcartel` (lib + tests) with behavior-preserving hand fixes, close the deferred `--`→`—` house-style item, and lock it in with a hard workspace clippy gate.

**Architecture:** Pure code-shape refactors — no runtime behavior change, no new tests (the existing 876-test suite staying green after each task IS the correctness proof). Fixes are hand-applied (no `clippy --fix`, no `cargo fmt`). The durable gate (`[workspace.lints.clippy] all = "deny"`) lands LAST, on an already-clean tree.

**Tech Stack:** Rust, `cargo clippy --all-targets`, Cargo workspace lints.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-06-30-wordcartel-clippy-debt-cleanup-design.md` (Codex-clean).
- **No `cargo fmt`** (ever). **No `clippy --fix`** — hand-apply each fix, matching the dense house style. Do not reflow untouched code.
- **Behavior-preserving:** after every task, `cargo test -p wordcartel-core -p wordcartel` stays green (baseline 876). No new tests (no new behavior).
- House style: em-dash `—` not `--` in prose comments; match surrounding style.
- **The fresh clippy run is authoritative.** Each crate task begins by running `cargo clippy -p <crate> --all-targets` and resolving EVERY finding — note clippy reports some lints at `error:` level (e.g. `reversed_empty_ranges`), NOT just `warning:`, so grep for both. The enumerated sites below are the guide (captured at plan time, merged-main @ 775161b); reconcile any delta. Known error-level delta: `change.rs:324`, `change.rs:405` (`reversed_empty_ranges`) — handled explicitly in Task 1 Step 3b (these are `#[allow]`, NOT a fix).
- **Two judgment calls** (`should_implement_trait`, `too_many_arguments`) — the implementer records the per-site choice + one-line rationale IN A CODE COMMENT and the task report; the Codex plan/pre-merge review adjudicates each. This plan gives the RECOMMENDED choice + complete code; the implementer may deviate with recorded reasoning.
- Every commit ends with the trailers, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

## File Structure & task map

- Task 1 — `wordcartel-core` lints (17 sites across block_tree, buffer, change, layout, md_parse, textobj; incl. judgment (a) `from_str`).
- Task 2 — `wordcartel` lints (27 sites across app, clipboard, config, derive, editor, export, filter, mouse, nav, panicx, registry, render, scratch, search_overlay, state, transform; incl. judgment (a) `next` + judgment (b) `too_many_arguments`).
- Task 3 — house-style `--`→`—` sweep.
- Task 4 — durable gate (`Cargo.toml` workspace lints + `[lints.rust]` migration) + CLAUDE.md flip.

---

### Task 1: `wordcartel-core` clippy warnings

**Files:** Modify `wordcartel-core/src/{block_tree.rs, buffer.rs, change.rs, layout.rs, md_parse.rs, textobj.rs}`.

**Interfaces:**
- Produces: `impl std::fmt::Display for TextBuffer` (replaces the inherent `to_string`; `.to_string()` still works via blanket `ToString`). Signature of `TextBuffer::from_str(&str) -> Self` is UNCHANGED (judgment (a) → `#[allow]`).

- [ ] **Step 1: Baseline the warnings**

Run: `cargo clippy -p wordcartel-core --all-targets 2>&1 | grep -E "warning:|error:|-->"`
Expected: the 17 `warning:` sites below PLUS the 2 `error:`-level `reversed_empty_ranges` at `change.rs:324`, `change.rs:405` (Step 3b) — 19 total.

- [ ] **Step 2: Apply the mechanical/semantic fixes (hand, per clippy's `help:` suggestion)**

| File:line | Lint | Fix |
|---|---|---|
| `block_tree.rs:442` | `ptr_arg` | change the `&mut Vec<_>` param to `&mut [_]` |
| `block_tree.rs:725,760,761` | `unnecessary_map_or` | apply clippy's `is_some_and`/`is_none_or`/`==` suggestion |
| `change.rs:637` | `vec_init_then_push` | build with the `vec![…]` literal clippy suggests |
| `layout.rs:743` | `collapsible_str_replace` | `replace(['\n', '\r'], " ")` (single multi-char pattern) |
| `md_parse.rs:154` | `ptr_arg` | `&mut Vec<_>` → `&mut [_]` |
| `md_parse.rs:175` | `manual_range_contains` | `(1..=6).contains(&n)` per clippy |
| `md_parse.rs:186,217,234,292,316,327` | `needless_range_loop` | see Step 3 (CARE) |
| `textobj.rs:37` | `filter_next` | `.rfind(..)` per clippy |

For each: apply the exact replacement clippy prints in its `help:` line, by hand, matching surrounding style. These are single-expression rewrites with zero behavior change.

- [ ] **Step 3: `needless_range_loop` ×6 in `md_parse.rs` (CORRECTNESS WATCH)**

Each is a `for b in 0..N { visible[b] = false; }`-style concealment write (verified at `md_parse.rs:186`: `for b in 0..content_start { visible[b] = false; }`). For a pure-write range, use `fill`:
```rust
visible[..content_start].fill(false);
```
For each of the 6 sites: FIRST confirm the loop index is used ONLY to index `visible` (a pure write, no read of `visible[b]`, no other use of `b`). If so, rewrite to the equivalent `visible[start..end].fill(false)` (adjust the range to the loop bounds). If any site also READS `visible[b]` or uses `b` for something else, use `for v in visible[start..end].iter_mut() { *v = false; }` instead, or keep the loop with `#[allow(clippy::needless_range_loop)]` + a one-line reason. Record which form each site got.

- [ ] **Step 3b: `reversed_empty_ranges` ×2 in `change.rs` tests (`change.rs:324`, `change.rs:405`) — `#[allow]`, do NOT flip**

These are INTENTIONAL reversed-range test inputs — `ChangeSet::delete(7..5, rev.len())` (:324) and `ChangeSet::delete(3..1, 5)` (:405) — that exercise `ChangeSet::delete`'s reversed-range NORMALIZATION. Flipping `7..5`→`5..7` would destroy the test. Instead, add a local `#[allow]` with a comment at EACH site (on the statement, or the enclosing test fn):
```rust
        // Intentional reversed range: exercises ChangeSet::delete's reversed-range
        // normalization. Do not "fix" the direction.
        #[allow(clippy::reversed_empty_ranges)]
        let cs_rev = ChangeSet::delete(7..5, rev.len()); // reversed
```
and likewise at `change.rs:405` (`let cs = ChangeSet::delete(3..1, 5);`). Confirm the two tests still pass unchanged.

- [ ] **Step 4: `inherent_to_string` → `Display` (`buffer.rs:100`)**

Delete the inherent `to_string` (buffer.rs:100-102) and add a `Display` impl (byte-identical output — both are the rope's `Display`). **Call surface:** `to_string` has production COLD-path callers (`wordcartel/src/export.rs:99` pandoc stdin, `wordcartel/src/workspace.rs:68` persist, `wordcartel/src/commands.rs:506`) PLUS test assertions — ALL keep working unchanged via the blanket `ToString` (none are on the per-keystroke path). The impl:
```rust
impl std::fmt::Display for TextBuffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.rope)
    }
}
```
Place it just after the `impl TextBuffer { … }` block. All existing `buf.to_string()` call sites keep compiling via the blanket `ToString`. Verify with `cargo build -p wordcartel-core`.

- [ ] **Step 5: Judgment call (a) — `from_str` (`buffer.rs:11`)**

`TextBuffer::from_str(s: &str) -> Self` is INFALLIBLE, so it cannot cleanly implement std `FromStr` (which returns `Result`), and a rename touches ~61 call sites (core, shell, tests, and the detached fuzz crate). **Recommended: `#[allow]` with a rationale** — attach directly to the method:
```rust
    // Named `from_str` for readability at 61 call sites; std `FromStr` is inapplicable
    // (this constructor is infallible — no `Result`/`Err`). Renaming buys nothing.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        TextBuffer { rope: ropey::Rope::from_str(s) }
    }
```
(The implementer may instead rename to `from_text` + migrate all ~61 sites incl. `wordcartel-core/fuzz/`, if judged clearer — recording the reasoning. Codex adjudicates.)

- [ ] **Step 6: Verify + commit**

Run: `cargo clippy -p wordcartel-core --all-targets 2>&1 | grep -cE "warning:|error:"` → Expected: `0` (the deliberate `#[allow]`s at `from_str` + the 2 reversed-range sites suppress their own findings).
Run: `cargo test -p wordcartel-core` → Expected: all green (222 lib + oracle).
`cargo build -p wordcartel-core` + `cargo test --no-run -p wordcartel-core` warning-free.
```bash
git add wordcartel-core/src
git commit -m "style(core): clear all clippy::all warnings (Display impl, needless_range_loop->fill, ptr_arg, from_str allow)"   # + trailers
```

---

### Task 2: `wordcartel` clippy warnings

**Files:** Modify `wordcartel/src/{app.rs, clipboard.rs, config.rs, derive.rs, editor.rs, export.rs, filter.rs, mouse.rs, nav.rs, panicx.rs, registry.rs, render.rs, scratch.rs, search_overlay.rs, state.rs, transform.rs}`.

**Interfaces:**
- Consumes: none from Task 1 (independent crate diff).
- Produces: `impl Default for CancelFlag` (delegates to `new()`); `apply_filter_done` signature UNCHANGED (judgment (b) → `#[allow]`); `SearchState::next` UNCHANGED (judgment (a) → `#[allow]`).

- [ ] **Step 1: Baseline** — `cargo clippy -p wordcartel --all-targets 2>&1 | grep -E "warning:|-->"` → the 27 sites below.

- [ ] **Step 2: Mechanical/semantic fixes (hand, per clippy's `help:`)**

| File:line | Lint | Fix |
|---|---|---|
| `app.rs:888` | `type_complexity` | extract a `type` alias for the complex type at that site; use it in the signature |
| `app.rs:1593` | `collapsible_if` | collapse the nested `if` per clippy |
| `app.rs:1710` | `unnecessary_unwrap` | use `if let Some(..) = editor.minibuffer` instead of `is_some()`+`unwrap()` |
| `app.rs:4026` | `redundant_pattern_matching` | `.is_some()` per clippy |
| `clipboard.rs:148` | `manual_div_ceil` | `input.len().div_ceil(3)` (or the exact operands) per clippy |
| `config.rs:407` | `bool_assert_comparison` | `assert!(x)` / `assert!(!x)` per clippy (test) |
| `derive.rs:137` | `len_zero` | `.is_empty()` |
| `editor.rs:216,446` | `unnecessary_map_or` | `is_some_and`/`is_none_or`/`==` per clippy |
| `export.rs:178` | `map_identity` | remove the identity `map_err` |
| `mouse.rs:164,233` | `manual_checked_ops` | see Step 3 (CARE) |
| `nav.rs:48` | `len_zero` | `.is_empty()` |
| `nav.rs:511` | `int_plus_one` | `li < total_lines` per clippy (see Step 3 CARE) |
| `nav.rs:714` | `filter_next` | `.rfind(..)` |
| `panicx.rs:75,84` | `redundant_closure` | replace the closure with the function itself per clippy |
| `registry.rs:158` | `redundant_closure` | replace the closure with the function itself |
| `render.rs:144` | `manual_clamp` | `.clamp(lo, hi)` — CONFIRM `lo <= hi` at the site before applying |
| `render.rs:1804,1810` | `unnecessary_cast` | drop the `as u16` (already `u16`) |
| `scratch.rs:110` | `option_map_unit_fn` | use `if let Some(..)` instead of `.map(..)` returning unit |
| `state.rs:286` | `field_reassign_with_default` | set the field in the struct initializer instead of after `Default::default()` |
| `transform.rs:326` | `needless_borrow` | remove the `&` that is immediately dereferenced |

- [ ] **Step 3: Arithmetic fixes (CORRECTNESS WATCH — preserve exact semantics)**

- `mouse.rs:164,233` (`manual_checked_ops`, "manual checked division"): clippy wants `checked_div`. Read each site; the rewrite must preserve EXACT behavior — a manual `if divisor != 0 { a / b } else { fallback }` becomes `a.checked_div(b).map_or(fallback, |v| v)` (or `.unwrap_or(fallback)`), yielding the SAME fallback on zero. Confirm the fallback value matches.
- `nav.rs:511` (`int_plus_one`): `x >= y + 1` → `x > y`, or `li >= total_lines + 1` → `li >= total_lines`… clippy's suggested form is `li < total_lines` (a negation context). APPLY EXACTLY clippy's printed suggestion and confirm the surrounding boolean sense is unchanged (watch for an inverted condition — re-read the enclosing `if`).

- [ ] **Step 4: `new_without_default` — `CancelFlag` (`filter.rs:74`)**

Add after the `impl CancelFlag { … }` block:
```rust
impl Default for CancelFlag {
    fn default() -> Self {
        CancelFlag::new()
    }
}
```

- [ ] **Step 5: Judgment call (a) — `SearchState::next` (`search_overlay.rs:129`)**

`pub fn next(&mut self) -> Option<Match>` is a WRAPPING BIDIRECTIONAL cursor: it pairs with `prev()`, sets `direction`/`wrapped`, and **never returns `None` once matches exist** (it wraps via `i % len`). That is the opposite of `Iterator::next` (forward-consuming, terminates with `None`); implementing `Iterator` would be an infinite, misleading abstraction. **Recommended: `#[allow]` with rationale**:
```rust
    // Wrapping bidirectional search cursor (pairs with `prev()`; wraps infinitely,
    // never yields None once matches exist) — NOT forward-consuming iteration, so
    // std `Iterator` is the wrong abstraction.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<Match> {
```
(Implementer may implement `Iterator` instead if judged appropriate — recording why. Codex adjudicates.)

- [ ] **Step 6: Judgment call (b) — `apply_filter_done` (`app.rs:274`, 8 args)**

Called from only the two `Msg::FilterDone` arms; a struct-of-args would duplicate the existing `Msg::FilterDone` fields for no clarity gain. **Recommended: `#[allow]` with rationale**:
```rust
// 8 args mirror Msg::FilterDone's fields 1:1; an args-struct would just duplicate
// that variant. Called from only the two FilterDone match arms.
#[allow(clippy::too_many_arguments)]
fn apply_filter_done(
```
(Implementer may refactor to a struct if judged clearer — recording why. Codex adjudicates.)

- [ ] **Step 7: Verify + commit**

Run: `cargo clippy -p wordcartel --all-targets 2>&1 | grep -c "warning:"` → Expected: `0` (the deliberate `#[allow]`s suppress their sites).
Run: `cargo test -p wordcartel` → all green (600+ lib).
`cargo build -p wordcartel` + `cargo test --no-run -p wordcartel` warning-free.
```bash
git add wordcartel/src
git commit -m "style(shell): clear all clippy::all warnings (Default impl, checked_div, allows for next/apply_filter_done)"   # + trailers
```

---

### Task 3: house-style `--`→`—` sweep

**Files:** Modify `wordcartel/src/app.rs` (the confirmed site) + any other confirmed prose site.

- [ ] **Step 1: Fix the confirmed site**

In `wordcartel/src/app.rs`, the user-facing status string (grep for it — currently ~line 797):
`"system clipboard unavailable -- copy/paste work in-editor; using OSC 52 for terminal sync"`
Change the `--` to `—` (em-dash):
```rust
        editor.status = "system clipboard unavailable — copy/paste work in-editor; using OSC 52 for terminal sync".into();
```
If any test asserts this exact string, update the assertion to match.

- [ ] **Step 2: Editorial grep for siblings**

Run: `grep -rn -- '--' wordcartel/src wordcartel-core/src | grep '"'`
Review each hit WITH JUDGMENT. Fix ONLY `--` that is a genuine prose em-dash inside a user-facing string or prose comment. Do NOT touch: markdown/test-fixture `--`/`---`, CLI flag args (`--config`, `--no-config`), shell-command examples, `// ----` separator-rule comments. Most hits are excluded; `app.rs`'s status string is the primary real one. Record which sites (if any) you changed and why.

- [ ] **Step 3: Verify + commit**

Run: `cargo test -p wordcartel` → all green (incl. any updated string assertion).
```bash
git add wordcartel/src wordcartel-core/src
git commit -m "style: em-dash in user-facing 'clipboard unavailable' status (house-style)"   # + trailers
```

---

### Task 4: durable clippy gate + CLAUDE.md flip

**Files:** Modify root `Cargo.toml`, `wordcartel-core/Cargo.toml`, `wordcartel/Cargo.toml`, `CLAUDE.md`. Do this LAST — the tree must already be clippy-clean.

- [ ] **Step 1: Move core's existing `[lints.rust]` to the workspace + add the clippy deny**

`wordcartel-core/Cargo.toml:23` currently has:
```toml
[lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(fuzzing)'] }
```
DELETE that `[lints.rust]` block from `wordcartel-core/Cargo.toml` (a crate cannot set both `[lints] workspace = true` and its own local `[lints.*]`). Add to the ROOT `Cargo.toml` a workspace lints section:
```toml
[workspace.lints.clippy]
all = "deny"

[workspace.lints.rust]
unexpected_cfgs = { level = "warn", check-cfg = ['cfg(fuzzing)'] }
```
(The `check-cfg` applied workspace-wide is harmless for the shell, which never uses `cfg(fuzzing)` — it only registers the name as known.)

- [ ] **Step 2: Opt each member crate in**

Add to BOTH `wordcartel-core/Cargo.toml` and `wordcartel/Cargo.toml` (top-level table):
```toml
[lints]
workspace = true
```

- [ ] **Step 3: Verify the gate**

Run: `cargo clippy --workspace --all-targets` → Expected: FINISHES CLEAN (zero warnings; would ERROR if any `clippy::all` warning remained — proving the deny is live).
Run: `cargo clippy -p wordcartel-core --all-targets 2>&1 | grep "unexpected_cfgs"` → Expected: still recognized (the migrated `check-cfg` works from the workspace; the M7 `cfg(fuzzing)` gate still passes).
Run: `cargo test -p wordcartel-core -p wordcartel` → all green (876).
Run: `cargo build --workspace` → clean (clippy lints don't affect `build`).

- [ ] **Step 4: Flip the CLAUDE.md carve-out**

In `/home/jkeim/projects/groundwords/CLAUDE.md`, replace the gate bullet at ~line 92 that currently reads:
> **No new clippy warnings in the files you touched.** Do NOT gate on a clean `cargo clippy --all-targets -- -D warnings` over the whole workspace — the codebase carries pre-existing clippy debt (tracked as its own pre-Effort-P cleanup). Check your own diff: `cargo clippy -p <crate>` and confirm none of the new findings point at lines you added or changed.

with:
> **Workspace clippy clean is a GATE.** The clippy-debt cleanup (2026-06-30) cleared all `clippy::all` warnings and enabled `[workspace.lints.clippy] all = "deny"`. `cargo clippy --workspace --all-targets` MUST pass clean before merge. New warnings fail the clippy run; deliberate exceptions require an item-local `#[allow(clippy::…)]` with a one-line rationale (never a blanket crate/workspace allow).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml wordcartel-core/Cargo.toml wordcartel/Cargo.toml CLAUDE.md
git commit -m "chore: enable [workspace.lints.clippy] all=deny gate + flip CLAUDE.md clippy carve-out"   # + trailers
```

---

## Self-Review

**Spec coverage:** mechanical set → Tasks 1-2 tables ✓; semantic set (`ptr_arg`, `type_complexity`, `new_without_default`, `manual_checked_ops`, `inherent_to_string`) → Tasks 1-2 Steps ✓; judgment (a) `from_str`+`next` → T1 Step 5 / T2 Step 5 (recommended `#[allow]` + rationale, Codex adjudicates) ✓; judgment (b) `apply_filter_done` → T2 Step 6 ✓; em-dash sweep → Task 3 ✓; durable gate + `[lints.rust]` migration + CLAUDE.md flip → Task 4 ✓; gate lands last on a clean tree ✓.

**Placeholder scan:** the mechanical-fix tables say "apply clippy's `help:` suggestion" — this is concrete (clippy prints the exact machine-applicable replacement at each site); full code is given for the non-obvious fixes (Display, Default, needless_range_loop→fill, the three judgment `#[allow]`s, the gate TOML, the CLAUDE.md text). No true placeholders.

**Type consistency:** `impl Display for TextBuffer` (T1) — `.to_string()` unaffected; `impl Default for CancelFlag` (T2); `#[allow]`s keep the three flagged signatures identical. No cross-task signature drift (each task is an independent per-crate/per-file diff).

**Correctness watch honored:** `needless_range_loop` (T1 S3), `manual_checked_ops`/`int_plus_one` (T2 S3), `manual_clamp` bounds (T2 table) each carry an explicit "confirm semantics" instruction.
