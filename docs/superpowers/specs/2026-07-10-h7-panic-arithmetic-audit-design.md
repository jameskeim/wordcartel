# H7 — Panic-safety & arithmetic-soundness audit (design)

**Date:** 2026-07-10
**Status:** Codex spec-review gate PASSED (round 2, READY) — awaiting human spec review before plan
**Scope:** one effort / one branch (`effort-h7-panic-arithmetic-audit`).
**Ground truth:** `.git/sdd/h7-approved-design.md` (the human-approved decisions this spec
realizes), `.git/sdd/h7-code-surface-inventory.md` (the panic/cast sweep) — every claim below
re-verified against the real post-H11/H14 tree for this spec: each site was re-located by its
enclosing symbol and its code shape re-read from source. Line numbers cited are
*current-as-of-writing* and will drift — implementers anchor on symbol names
(`workspaceSymbol`/`documentSymbol`/grep), never recorded lines.

---

## 1. ⚠ Findings for the human

**No blocking findings — all 8 core parse-path arithmetic sites are PROVEN-SAFE (8 / 0 / 0 across
PROVEN-SAFE / NEEDS-CLAMP / REAL-BUG); the shell defects are latent, not live.** Four
observations worth ten seconds of your attention, none requiring a decision:

1. **The core proofs are conditional on a shell-side contract** (I0, §4.1): the `(ChangeSet,
   Edit)` pair handed to the parser must describe the same old→new transformation. It is
   structurally enforced today — the three production `Edit` constructors build both halves from
   the same `(from, to, text)` values — but it is a cross-crate reading-level argument, not a
   compiler guarantee. The release clamps this effort adds are exactly the insurance for it.
2. **The `commands::build_multi_replace` defects are real but unreachable today**: the release
   panic path is *not* the `Edit`-arithmetic (which only mis-sizes a covering `Edit`, parse-path
   blast radius) — it is the `ChangeSet` half. A malformed edit list makes the op-builder
   over- or under-consume the document, and `ChangeSet::from_ops` then trips its own release
   `assert!(retain + delete == len_before, …)` (change.rs:106). So *any* malformed list —
   empty, reversed, overlapping, or out of bounds — is a latent release panic, not just the
   empty case. All production callers honor the ascending / non-overlapping / in-bounds /
   non-empty contract today (§1.4), so nothing hits it; the fix (§4.3a) hardens the whole
   boundary before Effort P exposes it to plugins.
3. **One deviation from the approved design's letter, honoring its intent** (§4.3): the existing
   `debug_assert!(!edits.is_empty())` in `build_multi_replace` is *replaced* by a single
   well-formedness guard that treats **any** malformed list (empty, reversed, overlapping, or
   out of bounds — not just empty) as a no-op. The design's own regression tests ("empty and
   reversed/unordered edit lists → no panic") run under `cargo test` (debug), where a retained
   `debug_assert` — or a bare `from_ops` on a reversed list — would itself panic; the graceful
   guard is what lets those tests actually pass. The dev-loudness moves into the guard's
   documented no-op return.
4. **Full production caller inventory** (grounds "unreachable today"): a fresh
   `build_multi_replace(` grep gives seven production call expressions across four caller
   buckets, all satisfying the contract — `scratch.rs [append_to_scratch]`,
   `scratch.rs [move_block_to_scratch]` (single
   `(b.start, b.end, …)` pair); `blocks_marked.rs` (×3: a single insert pair, a single
   delete pair, and a pre-sorted ascending list); `search_ui.rs [search_replace_all]` :60 and
   `search_ui.rs [search_step_rest]` :117 (both fed ascending, non-overlapping match spans from
   `search::…`; `search_step_rest` additionally early-returns on `edits.is_empty()` @:116
   *before* calling). Every other `build_multi_replace(` hit is `#[cfg(test)]` (`editor.rs`,
   `derive.rs`, `session_restore.rs`, `workspace.rs`, `commands.rs`'s own test module). So no
   live path reaches the malformed guard — it is pure boundary insurance.

---

## 2. Governing stance — blast-radius calibration

Guard strength matches what breaking the invariant costs the *user*, not a blanket fail-loud
policy. The codebase already encodes the split; H7 makes the parse path's RELEASE behavior
consistent with it and records the verdicts.

- **Mutation path** (`TextBuffer` in `wordcartel-core/src/buffer.rs`, `ChangeSet` in
  `change.rs`): a bad position corrupts the document — the cardinal sin. The release `assert!`s
  (buffer.rs `[TextBuffer::apply]`-adjacent char-boundary/range checks at :40/:49/:54/:65/:70;
  change.rs `[ChangeSet::insert]` :46, `[ChangeSet::delete]` :62/:74, `[ChangeSet::from_ops]`
  :106) are correct, forever. H7 changes none of them; §6 records the verdict. The
  change.rs:31–40 invariant comment (plain usize arithmetic on the trusted hot path, bounded by
  the ~5 MB doc cap ~12 orders below `usize::MAX`, malformed positions caught loudly by
  TextBuffer's release asserts) is the standing policy every core remediation below reconciles
  with — no checked/Result-propagating arithmetic is introduced.
- **Incremental parse path** (`block_tree.rs` region math): a bad number only mis-sizes the
  reparse region; the rope is untouched; the failure is a cosmetic highlight/fold glitch that
  self-heals on the next reconcile (the H3 class — blast radius ≈ a wrong color for ~150 ms).
  Additionally, the whole incremental call is already wrapped in `panicx::catch` at its sole
  production call site (`derive.rs [rebuild]`, the `incremental_update_instrumented_src_owned`
  dispatch), so even a wrap-induced slice panic today degrades to the empty-tree fallback rather
  than crashing — the release clamps below turn that degraded-frame outcome into a merely
  over-wide (still correct) reparse. Policy per site: *prove* non-negativity from the region
  invariants, keep the `debug_assert!` as the dev/fuzz guard (the F2 oracle also patrols
  incremental≡full), and make the release fall-through degrade by clamping instead of wrapping.
- **Shell cursor-placement casts** (`render.rs [place_cursor]`, `nav.rs [screen_pos]`): a
  mis-clamped cursor is cosmetic by the same logic — SILENT saturating clamp, no `debug_assert`.

Rejected alternatives (settled in the approved design; recorded so the plan doesn't relitigate):
no blanket saturating helper that *silently* clamps everywhere (the clamp is the release
fall-through **behind** a `debug_assert`, not a replacement for it); no checked/Result arithmetic
in core (contradicts the change.rs hot-path policy and the doc-cap bound).

---

## 3. Scope & non-goals

**In scope:** (a) the per-site arithmetic verdicts + remediations of §4; (b) the two
`commands.rs [build_multi_replace]` panic fixes; (c) the recorded panic-verification verdict of
§5 with the full classification appendix of §6; (d) the core-crate cast-lint gate of §7;
(e) the guardrail tests of §9.

**Non-goals (explicit, from the approved design):**
- **No `BytePos` newtype.** `lib.rs:29 pub type BytePos = usize;` stays a bare alias; no typed
  position wrapper with checked arithmetic is introduced. Clamp/saturate is applied at call
  sites (via one file-local helper in `block_tree.rs`, §4.2 — a helper is not a newtype).
- **No bulk `.unwrap()` → `.expect()` conversion** of the 57 guarded sites (§6). They are
  correct; converting them is busywork that churns blame.
- **No change to the mutation-path release `assert!`s** in `buffer.rs`/`change.rs` — verdict
  recorded (§2, §6), code untouched.
- **The shell's ~70 terminal-coordinate widening casts stay unannotated** — the cast-lint gate
  covers `wordcartel-core` only (§7). The fixed `render.rs`/`nav.rs` sites are corrected but not
  lint-gated.
- No behavior change anywhere: every remediation is either dead code under the proven invariant
  (release clamps that never engage) or a hardening of an unreachable-today boundary
  (`build_multi_replace`). Existing tests pass unchanged.

---

## 4. Arithmetic remediation

### 4.1 The invariants the proofs stand on

All core sites live in `wordcartel-core/src/block_tree.rs`, in or under
`incremental_update_instrumented_src_owned` (the one production entry; the `&`/owned/rope
wrappers `incremental_update`, `incremental_update_instrumented`, `incremental_update_rope`,
`incremental_update_instrumented_src` all delegate to it). Notation: `edit_lo =
edit.range.start`, `edit_hi = edit.range.end`, `delta = edit.delta() = new_len − range.len()`
(signed).

- **I0 — edit faithfulness (caller contract, shell-side).** `edit_lo ≤ edit_hi ≤ old_src.len()`
  and `new_src.len() = old_src.len() − (edit_hi − edit_lo) + new_len`, i.e.
  `new_src.len() = old_src.len() + delta`. Established structurally: the three production `Edit`
  constructors — `commands.rs [build_range_replace]`/`[build_multi_replace]`,
  `commands/edit.rs` (eight primitives, each `Edit { range: from..to, new_len: text.len() }`
  beside the matching `ChangeSet`), `transact.rs` (whole-doc `0..len_before → len_after`) —
  build the pair from the same values; `ChangeSet::from_ops`/`Buffer::apply` release-assert the
  changeset against the live buffer; `derive.rs [rebuild]` only takes the incremental path when
  the tree is exactly one version behind and `pre_edit_rope`+`last_edit` bridge that gap.
- **I1 — enclosure.** `region_old_start ≤ edit_lo` and `region_old_end ≥ edit_hi`, established
  at the `region_old_start = region_old_start.min(edit_lo)` /
  `region_old_end = region_old_end.max(edit_hi)` pair early in the function and *maintained by
  every subsequent mutation* (verified line by line): the line-boundary snaps only move outward;
  the group-walk, container pull-back and straddle repair only decrease `start` / increase
  `end`; `apply_local_slack` sets `end` to the next-next block's `span.start ≥ region_old_end`
  (sorted tops) or to `old_src.len()`; the widen arms set `end` to `old_src.len()` or to a
  `base_end` copied after growth-only transforms; the `fm_floor` max cannot lift `start` past
  `edit_lo` (gated on `edit_lo ≥ floor`). Documented by the `debug_assert!`s
  `region_old_start <= edit_lo` / `region_old_end >= edit_hi` (twice, the second after the
  front-matter floor) in the same function.
- **I2 — in-bounds.** `region_old_end ≤ old_src.len()`: every growth path lands on a block
  `span.end ≤ len` (parser-produced spans), a `line_end(·) ≤ len`, or `len` itself.
  Machine-checked by the "Gap 3" `debug_assert!(region_old_end <= old_src.len(), …)`.
- **T — tree well-formedness.** Top-level blocks sorted by start, non-overlapping; children
  spans contained in their parent's span. Parser-produced (pulldown-cmark event nesting via
  `parse_region`/`full_parse_src`) and inductively preserved by `shift_in_place`/splice.
  Documented by the splice-ordering `debug_assert!(splice_lo <= splice_hi, …)`.
- **Lemma L1 (the workhorse).** For any `X ≥ edit_hi`:
  `X + delta = X − (edit_hi − edit_lo) + new_len ≥ edit_lo + new_len ≥ 0`.
  Under I2 additionally `X + delta ≤ old_src.len() + delta = new_src.len()` (by I0). So every
  `(X as isize + delta) as usize` with `X ≥ edit_hi` is non-negative *and* ≤ `new_src.len()`.
- **isize-conversion soundness.** `len as isize` cannot wrap: every operand is a length/offset
  of an in-memory document (≤ the ~5 MB cap per the change.rs policy; unconditionally
  ≤ `isize::MAX` by Rust's allocation-size guarantee).

### 4.2 Core parse path — per-site verdicts

Eight sites; every one PROVEN-SAFE. "Clamped?" = whether the release fall-through today degrades
safely if the proof's preconditions were violated (I0 broken by a future caller bug).

| # | Site (symbol anchor) | Expression shape | Clamped today? | Verdict | Proof chain | Remediation |
|---|---|---|---|---|---|---|
| 1 | `Edit::delta` (:468) | `new_len as isize − range.len() as isize` | n/a (signed by design) | **PROVEN-SAFE** | This is the *deliberate* signed delta, not a bug. Conversions can't wrap (§4.1). Consumers verified: sites 2–8 below plus `shift_in_place` — all treat it as signed and add it to a `usize ≥ edit_hi` position (L1) or a tree span (T). | Keep. Gains the item-local cast-lint `#[allow]` + one-line doc-cap reason (§7). |
| 2 | `incremental_update_instrumented_src_owned` — `base_new_end` (:870) | `(base_end as isize + delta) as usize`, then `.saturating_sub(base_start)` | Effectively: consumed only via `saturating_sub` into a size compared against `MAX_SYNC_WIDEN_BYTES`; a hypothetical wrap ⇒ huge size ⇒ Case B full widen (correct, slower) | **PROVEN-SAFE** | `base_end` is copied from `region_old_end` (I1: ≥ `edit_hi`) then only grown by `apply_local_slack`/`repair_region` ⇒ `base_end ≥ edit_hi` ⇒ L1. Upper: `base_end ≤ old_src.len()` (I2 arguments) ⇒ result ≤ `new_src.len()`. | Route through the shared `shift_offset` helper (below) for uniformity + lint conformance; consumption unchanged. |
| 3 | same fn — first `region_new_end` (:929) | `(region_old_end as isize + delta) as usize`, feeds `new_src.slice(region_new_start..region_new_end)` | **UNCLAMPED** (a wrap ⇒ rope-slice panic ⇒ caught by `panicx::catch` in `derive::rebuild` ⇒ degraded empty-tree frame) | **PROVEN-SAFE** | I1 gives `region_old_end ≥ edit_hi` (debug-asserted two lines above) ⇒ L1 ⇒ ≥ `edit_lo + new_len ≥ 0`. Lower bound vs. slice start: `region_new_start = region_old_start ≤ edit_lo ≤ edit_lo + new_len ≤ region_new_end`. Upper: I2 ⇒ ≤ `new_src.len()`. | `shift_offset` (debug_assert + clamp-at-0) **plus** explicit release band-clamp to `region_new_start..=new_src.len()` so a violated I0 degrades to an over-wide-but-valid reparse instead of the panic→fallback path. |
| 4 | same fn — second `region_new_end` (:958, created-container re-widen) | same shape, after `region_old_end = old_src.len()` | **UNCLAMPED** (same panic-then-fallback story) | **PROVEN-SAFE** | `region_old_end = old_src.len() ≥ edit_hi` (I0) ⇒ L1; and `old_src.len() + delta = new_src.len()` exactly (I0) — the slice is precisely the document tail. | Same as site 3. |
| 5 | `needs_widen_to_end` (:1077) | `((region_old_end as isize + edit.delta()) as usize).min(new_src.len())`, slice via `new_start.min(new_region_end)..new_region_end` | **Already clamped both ways** (`.min(len)` upper; the `min` inside the slice guards inversion) — a hypothetical wrap ⇒ huge ⇒ `.min(len)` ⇒ over-wide scan ⇒ conservative widen (safe) | **PROVEN-SAFE** | Called with the same `region_old_end` as site 3 context (≥ `edit_hi` by I1 at the call site) ⇒ L1. | Keep shape; route the raw cast through `shift_offset` for lint conformance only. |
| 6 | `html_in_play` (:1119) | identical shape to site 5 | already clamped | **PROVEN-SAFE** | Called with post-enclosure `region_old_start/end` (I1 holds at the call) ⇒ L1. | Same as site 5. |
| 7 | `region_has_bare_cr` (:1142) | identical shape to site 5 | already clamped | **PROVEN-SAFE** | Same call context as site 6 ⇒ L1. | Same as site 5. |
| 8 | `shift_range` (:1290, via `shift_in_place`) | `((r.start as isize + delta) as usize)..((r.end as isize + delta) as usize)` | **UNCLAMPED — and the result *persists* into tree state** (a wrapped span would live in `blocks()` until the next reconcile, feeding render/outline garbage) | **PROVEN-SAFE** | Applied only to after-blocks selected by `partition_point(span.start ≥ region_old_end ∧ span.end > region_old_end)` ⇒ every top-level `r.start ≥ region_old_end ≥ edit_hi` ⇒ L1; recursion into children is covered by T (children ⊆ parent span ⇒ same lower bound); upper bound `r.end ≤ old_src.len()` (parser spans) ⇒ shifted ≤ `new_src.len()`. | `shift_offset` on both endpoints: `debug_assert` non-negativity, release clamp at 0. No upper clamp — `len` is not in scope and the L1/I0 upper bound plus reconcile self-healing make threading it through pure ceremony. |

**Bucket counts: PROVEN-SAFE 8 · NEEDS-CLAMP 0 · REAL-BUG 0.** The release clamps added at
sites 3/4/8 are *defense in depth for I0* (the one cross-crate, reading-level precondition), not
corrections — under the proven invariants they are dead code.

**The `shift_offset` helper (remediation vehicle, not a policy change).** One file-local fn in
`block_tree.rs`, shape (described, not final code — the plan writes it):
`fn shift_offset(pos: usize, delta: isize) -> usize` — `debug_assert!` that
`pos as isize + delta ≥ 0` (message naming the region invariant), release fall-through
`.max(0)`-style clamp, internal casts carrying the single item-local
`#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]` + one-line doc-cap reason. This
is the approved synthesis (debug_assert stays the dev/fuzz guard, the clamp is only the release
fall-through), *not* the rejected silent-saturating helper — the assert lives inside it. Using
it at all seven expression sites (2–8) concentrates the cast-lint allows into exactly two
carriers in `block_tree.rs` (`Edit::delta` + `shift_offset`) instead of nine scattered ones.
Existing upper clamps (`.min(new_src.len())` at sites 5–7) stay at their call sites; sites 3/4
gain the band-clamp described in the table; the `debug_assert!`s at the region checkpoints
(`region_old_end <= old_src.len()`, the two enclosure asserts, the splice-order assert) all stay
— strengthened only in that `shift_offset` now also asserts per-application.

### 4.3 Shell sites — real fixes

**(a) `commands.rs [build_multi_replace]` — malformed edit list panics (empty slice, and any
non-well-formed list via `from_ops`).** Two release panic surfaces, both from a caller-contract
violation (verified against source):
- *Empty slice:* `edits.first().unwrap().0` / `edits.last().unwrap().1` (:147–148) release-panic
  on an empty list; today backed only by `debug_assert!(!edits.is_empty())` (:137).
- *Any non-well-formed list is a `ChangeSet` panic — the primary surface.* The op-builder loop
  emits `Retain(from − pos)` / `Delete(to − from)` / `Insert` per edit and advances `pos = *to`,
  then a trailing `Retain(doc_len − pos)`. For a reversed pair like `(from=10, to=5)`: it retains
  10, sets `pos = 5`, then retains `doc_len − 5` — **over-consuming the old document**.
  `ChangeSet::from_ops` (change.rs:97) sums retains+deletes and hits its own release
  `assert!(retain + delete == len_before, …)` (change.rs:106). So a reversed, overlapping, or
  out-of-bounds list panics there in release — **not** in the `Edit` arithmetic. (The
  `(t − f)`/`last_to − first` subtractions at :150–151 additionally wrap, but that only mis-sizes
  the covering `Edit`, a parse-path blast radius — a secondary concern behind the `from_ops`
  panic.) Correcting the `Edit` arithmetic alone would therefore leave the real panic live and
  the §9 "reversed pair → no panic" test failing.

**Fix — one up-front well-formedness guard; malformed ⇒ identity no-op.** Consistent with the
blast-radius stance (§2) and the approved "an empty list is a no-op" precedent, the safe
degradation for a malformed multi-replace is to do **nothing** — it can neither panic nor
corrupt, and no production caller ever reaches it (§1.4). Before building any ops,
`build_multi_replace` validates that `edits` is a well-formed sequence: **non-empty**, each
`from ≤ to`, **ascending and non-overlapping** (`prev_to ≤ from` across the slice), and
**in-bounds** (`last_to ≤ doc_len`). If the check fails on any count, return the documented
identity no-op pair — a no-op `ChangeSet` (`Retain(doc_len)` when `doc_len > 0`, else empty ops)
paired with `Edit { range: 0..0, new_len: 0 }` — so a subsequent `apply` is a genuine no-op. Only
a validated list proceeds to the existing op-builder and covering-`Edit` computation, where all
the subtractions are now provably non-negative (guaranteed by the guard, so they need no
saturating dressing — the guard, not per-op clamping, is the single point of truth). This
**replaces** the `debug_assert!(!edits.is_empty())` (see §1.3): "empty" and "malformed" collapse
into one guard, the doc comment states the contract ("ascending, non-overlapping, in-bounds; a
malformed or empty list is a no-op"), and the dev signal becomes a documented total function
rather than a debug-only crash that would itself fail the §9 tests. The guard's cost is one
linear pass over a slice that is 1–few elements at every production call site (§1.4) — off any
hot loop.

**(b) `render.rs [place_cursor]` — truncate-then-guard cursor casts.** Verified shape: the
search-bar arm computes `x_offset = (prefix_cols + caret_cols) as u16` *then* guards
`x_offset < w`; the minibuffer arm likewise casts `prompt_cols`/`text_cols` to u16 before
summing/guarding. A ≥ 65536-char field therefore truncates *first* and can pass the guard at a
wrong column (cosmetic mis-placement instead of the intended pin/hide). Fix: compute the column
in `usize`, guard against `w as usize` **before** any narrowing, and cast only the proven-small
value — a silent clamp/guard reorder, **no `debug_assert`** (per §2, cursor placement is
cosmetic). The third expression in this fn (`(tg.text_width as usize).saturating_sub(1) as u16`)
is verified bounded (u16 → usize → −1 → u16) and untouched.

**(c) `nav.rs [screen_pos]` — `(vcol as u16, final_row as u16)`.** Verified: `final_row` is
already bounded (`final_row < area_height`, which is derived from the u16 terminal height, so
the row cast cannot truncate); `vcol` is *not* bounded — a no-wrap line can place the caret past
65 535 visual columns, truncating to a small column that then *passes* every downstream guard.
Fix: saturating narrow for the column (`u16::MAX` on overflow) so the existing D2 clamp in
`place_cursor` (`col.min(text_width − 1)`) pins the caret at the text edge — the correct visual
for "caret is far off-screen". Silent clamp, no `debug_assert` (same cosmetic calibration).

---

## 5. Panic-verification verdict (recorded conclusion)

The panic half of H7 is a *verification pass*, ~90% already satisfied; this section records the
verdict so future audits read it instead of re-deriving it. Population re-counted on the current
tree for this spec (per-file counts match the inventory exactly; every site re-anchored by
symbol in §6):

- **59 production `.unwrap()`** — 57 GUARDED-INVARIANT (correct as-is, §6.1; the guard
  establishing each invariant is verified present and adjacent), 2 UNCLEAR in
  `commands.rs [build_multi_replace]` (subsumed by the §4.3a well-formedness guard — the
  `.first()/.last()` unwraps sit behind it on the validated path, so they can no longer see an
  empty slice). Zero fallible/external unwraps.
- **13 production `.expect(…)`** — all informative, all correct as-is (§6.2): 5 thread-spawns
  (fallible OS path, startup-fatal by design — a machine that cannot spawn the input thread
  cannot run the editor), 8 invariant assertions with accurate messages.
- **1 production `panic!`** — `app.rs [reduce]`: the `WCARTEL_SMOKE_PANIC` deliberate
  crash-handler smoke hook, triple-gated (`#[cfg(debug_assertions)]` + F12 press + env var).
  Compiled out of release entirely. Correct as-is; the PTY smoke suite depends on it (S7).
- **0 production `unreachable!`/`todo!`/`unimplemented!`** (the latter two are workspace-denied
  lints).
- **Mutation-path release `assert!`s** (buffer.rs ×5, change.rs ×4): correct, forever — see §2.
  Untouched.

Net code change from the panic half: the single `build_multi_replace` well-formedness guard
(§4.3a), which subsumes both flagged unwraps and closes the `from_ops` panic. Nothing else.

---

## 6. Classification appendix — the full production panic-surface table

Anchors are `file [symbol]` with current-as-of-writing line numbers; symbols govern.
"Invariant" = the condition that makes the site infallible, verified present in source.

### 6.1 `.unwrap()` — 59 sites

**Overlay-dispatch guards (30).** Structural fact (verified): every overlay `intercept(…)`
opens with `if editor.<overlay>.is_none() { return Handled::Pass(msg); }` and is called
unconditionally from the `app::reduce` intercept chain; `mouse.rs [route_overlay]` guards each
region arm with `if editor.<overlay>.is_some()`. Within those scopes, `as_ref/as_mut/take`
unwraps are guarded by construction.

| Sites | Anchor | Invariant |
|---|---|---|
| mouse.rs:111,117 | `[route_overlay]` palette arm | `palette.is_some()` guard @:90 |
| mouse.rs:164,165,171,177,184 | `[route_overlay]` menu arm | `menu.is_some()` guard @:142 |
| mouse.rs:212,216 | `[route_overlay]` theme-picker arm | guard @:196 |
| mouse.rs:255,259 | `[route_overlay]` file-browser arm | guard @:240 |
| mouse.rs:295,299 | `[route_overlay]` outline arm | guard @:279 |
| mouse.rs:348,352 | `[route_overlay]` diag arm | guard @:331 |
| render_overlays.rs:329,345,354 | `[paint]` menu branch | menu-open branch condition |
| diag_overlay.rs:83,84 | `[intercept]` | `diag.is_none()` early-return @:79 |
| minibuffer.rs:81,84,87,90,104 | `[intercept]` | `minibuffer.is_none()` early-return @:74 |
| search_ui.rs:242,243,263,264,265,266 | `[intercept]` | `search.is_none()` early-return @:222 |
| prompts.rs:39 | `[intercept]` | prompt None-guard (early-return above) |
| file_browser.rs:91 | `[file_browser_enter]` | called only with browser open |
| app.rs:145 | `[hydrate_overlays]` | `menu.as_ref().is_some_and(…)` guard @:143 |

**`list_nav_key(c).unwrap()` (4)** — invariant = the enclosing match arm's own pattern guard
`c if list_nav_key(c).is_some()`: palette.rs:142 `[intercept]`, file_browser.rs:127
`[intercept]`, outline_overlay.rs:89 `[intercept]`, theme_picker.rs:73 `[intercept]`.

**Cursor/char-iterator unwraps (5)** — invariant = the adjacent cursor bounds check on the same
string: minibuffer.rs:54 `[Minibuffer::left]` `next_back` (guard `cursor > 0`), :62
`[Minibuffer::right]` `next` (guard `cursor < text.len()`); palette.rs:169 `[intercept]` `next`
(guard `cursor < query.len()`); search_overlay.rs:82 `[left]` `next_back` (guard `cursor > 0`),
:83 `[right]` `next` (guard `cursor < len`).

**Other guarded (18):**

| Site | Anchor | Invariant |
|---|---|---|
| input.rs:32 | `[handle_key]` `filter_in_flight.take()` | `is_some()` guard @:31 |
| jobs_apply.rs:31 | `[apply_result]` `pending_after_save.as_ref()` | reached only when `fire` (derived from the same `Option`) is true |
| jobs_apply.rs:180 | `[drive_quit_drain]` `.position().unwrap()` | preceding `gone`/`is_dirty` check + `continue` guarantees the id is present |
| jobs_apply.rs:182 | `[drive_quit_drain]` `quit_drain.as_ref()` | loop entered only with drain Some |
| scratch.rs:21 | `[append_to_scratch]` `by_id_mut(sid)` | `sid` validated by the two let-else guards @:12–13 |
| base16.rs:64 | `[parse_base16]` `slot.unwrap()` | `extra.iter().all(is_some)` guard @:62 |
| keymap.rs:97 | `[parse_chord]` `f[1..].parse()` | match-arm guard `parse::<u8>().is_ok()` |
| keymap.rs:99 | `[parse_chord]` `chars().next()` | match-arm guard `chars().count() == 1` |
| keymap.rs:475 | `[build_keymap]` `preset_bindings("cua")` | known-builtin const preset |
| render.rs:706,712 | `[row_spans_placed]` `run_style.unwrap()` | `!run.is_empty()` ⇒ `run_style` was set when the run was started |
| theme_resolve.rs:21 | `[detect_depth]` `term_l.unwrap()` | preceding match returns on the None/empty arms (comment in source: "not None per the match above") |
| history.rs:152 | `[History::commit]` `revisions.last_mut()` | inside `if can_merge`, derived from an existing last revision |
| theme.rs:80 | `[rgb_to_named16]` `min_by_key(…).unwrap()` | `NAMED` is a non-empty const array |

**UNCLEAR → FIXED (2):** commands.rs:147,148 `[build_multi_replace]` `edits.first()/.last()` —
release-panic on an empty slice, previously backed only by a `debug_assert`. Both now sit behind
the §4.3a well-formedness guard (which also closes the larger `from_ops` panic on any malformed
list), so the validated path they run on can never be empty.

### 6.2 `.expect(…)` — 13 sites, all informative, all correct as-is

| Site | Anchor | Class / establishing invariant |
|---|---|---|
| app.rs:602 | `[App::…diag warmup spawn]` "spawn diag warmup thread" | OS thread-spawn; startup-fatal by design |
| app.rs:648 | input thread spawn | same |
| app.rs:656 | input watchdog spawn | same |
| clipboard.rs:224 | `[spawn_worker]` "spawn clipboard worker" | same |
| jobs.rs:133 | `[spawn_worker]` "spawn jobs worker" | same |
| export.rs:142 | `[pandoc_argv]` "WritesOutput requires an out path" | caller passes `out` for `WritesOutput` formats |
| file.rs:123 | `[open]` "already verified by is_binary" | UTF-8 validity pre-checked on the same bytes |
| save.rs:144 | `[do_save]` "do_save called without a path" | dispatcher routes pathless saves to Save-As |
| save.rs:224 | `[reload_from_disk]` "new_from_text yields one buffer" | constructor postcondition |
| save.rs:270 | `[load_recovered]` same | same |
| theme_resolve.rs:205 | `[resolve_theme]` "flexoki-dark is a bundled builtin" | bundled-theme constant |
| transform.rs:103,109 | `[transform_unit_at]` "path is never empty" | path built with ≥ 1 element in the same fn |

### 6.3 `panic!` family — 1 site

app.rs:230 `[reduce]` — `panic!("WCARTEL_SMOKE_PANIC: …")`, gated `#[cfg(debug_assertions)]`
@:224 ∧ `KeyCode::F(12)` press ∧ `WCARTEL_SMOKE_PANIC` env. Deliberate crash-handler smoke hook
(PTY suite S7: panic → restore → recovery dump). Compiled out of release. **Leave as-is.**

---

## 7. Enforcement gate — core-crate cast lints

**What:** `clippy::cast_possible_truncation`, `clippy::cast_sign_loss`,
`clippy::cast_possible_wrap` at `deny`, in `wordcartel-core` ONLY (where `#![forbid(unsafe_code)]`
already sets the higher bar and the offset math concentrates). All three are `clippy::pedantic`
members, so they are OFF today despite `[workspace.lints.clippy] all = "deny"` — which is why
the tree's 146 production casts carry zero allows.

**How — grounded correction to the anticipated mechanism.** Both crates declare
`[lints] workspace = true` (verified: `wordcartel-core/Cargo.toml` and `wordcartel/Cargo.toml`),
and Cargo rejects a manifest that sets `workspace = true` alongside any other key in the same
`[lints]` table — so a per-package `[lints.clippy]` block in `wordcartel-core/Cargo.toml` would
require abandoning inheritance and forking the whole workspace lint table into the crate (two
sources of truth for the shared denies). Instead the gate is a **crate-level attribute in
`wordcartel-core/src/lib.rs`**, beside the existing `#![forbid(unsafe_code)]`:
`#![deny(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)]`
with a one-line comment naming H7. Semantics are identical for our purposes: it rides the same
`cargo clippy --workspace --all-targets` merge gate, and item-local `#[allow(…)]` overrides it —
the exact precedence idiom the workspace-denied `too_many_lines`/`print_stdout` allows already
rely on. `[lints] workspace = true` stays untouched in both crates.

**The allow budget (small, verified by enumerating core's production casts):** the gate's
entire fallout in `wordcartel-core` is ~20 cast expressions in three files — `block_tree.rs`
(the 9 §4.2 expressions, collapsed by `shift_offset` to two reason-carrying allow sites:
`Edit::delta` and the helper; plus `[tag_to_kind]` :264 `*level as usize as u8`, HeadingLevel ∈
1..=6, allow + reason), `theme.rs` (channel math: `[luma-avg]` :91 sum/3 ≤ 255 → u8;
`[dist2]` :115 `(v*v) as u32` with v² ≥ 0; `[blend]` :411 and `[hsl]` :955/:968–970 f32→u8 on
`.clamp(0.0,255.0)`/[0,1]-scaled values — allow + one-line bound per carrying fn), and
`search.rs` :168 (`(d − b'0') as usize`, a pure widening — no lint fires, no allow). Every allow
is item-local with a one-line reason (never crate/module-blanket beyond the gate attribute
itself); the plan enumerates the final list from the clippy run.

**The shell is NOT gated.** Its ~70 u16↔usize terminal-coordinate casts stay unannotated; the
§4.3b/c fixes are corrections, not lint conformance. Extending the gate to the shell is a
possible future ratchet, deliberately out of H7's scope.

---

## 8. Command-surface-contract conformance

**N/A — H7 does not touch the command surface.** No command, user-settable option, palette
entry, menu item, or keybinding hint is added, removed, or changed; `build_multi_replace` is an
internal changeset builder behind existing commands, and its hardening changes no command
semantics (all in-tree callers already satisfy the guarded contract, §1.4). The contract's
invariant tests run unchanged in the merge gate.

---

## 9. Testing approach

- **Core guardrail tests (clamp-not-wrap near the boundary).** In `block_tree.rs`'s test
  module: (a) boundary-shaped *valid* edits through the public entries — delete-everything
  (`0..len → ""`, the most-negative delta), insert-into-empty, replace-at-EOF, whole-doc
  replace — asserting `incremental ≡ full` and in-bounds regions (these drive L1 at its
  extremes: `region_new_end` exactly 0 and exactly `new_src.len()`); (b) direct `shift_offset`
  tests at the exact boundary (`pos + delta == 0` → 0, positive sums exact). Inputs that
  *violate* non-negativity are deliberately untestable under `cargo test` (the `debug_assert`
  fires first — that is the design working); the release clamp is documented dead-code
  insurance, review-verified, not test-driven.
- **`build_multi_replace` regression tests** (per the approved design). Each asserts the malformed
  input degrades to the **identity no-op** — the returned `ChangeSet` applies with no document
  change and the `Edit` is `0..0 / new_len 0` — so the path provably does not reach
  `ChangeSet::from_ops` (whose release assert is the panic being closed): (a) empty list; (b)
  reversed pair `(from=10, to=5)` — the exact shape that would over-consume the doc and trip
  `from_ops`'s `retain + delete == len_before` assert; (c) overlapping/unordered list; (d)
  out-of-bounds `last_to > doc_len`. A companion positive test confirms a well-formed ascending
  multi-edit still produces the correct covering `Edit` and non-no-op `ChangeSet` (the guard does
  not reject valid input). All runnable in debug because the guard **replaces** the
  `debug_assert!(!edits.is_empty())` (§1.3, §4.3a) — nothing panics on the malformed inputs the
  tests feed.
- **Cursor-clamp tests.** `place_cursor` guard-order: a search/minibuffer field long enough
  that the pre-fix code would truncate-then-pass — assert the cursor is hidden/pinned, not
  mis-placed (via the existing `render_capturing_cursor` harness); `screen_pos` saturation: a
  no-wrap line with caret past u16::MAX visual columns → column saturates and the D2 clamp pins
  it at the text edge. Sized to stay cheap (one long-line buffer each).
- **F2 fuzz oracle unchanged** — it still owns incremental≡full divergence detection, and the
  strengthened per-application `debug_assert` in `shift_offset` now fires under fuzz (built with
  debug assertions) exactly where the region invariants would break.
- **Merge gates:** full `cargo test` green across all suites; `cargo build` / `cargo test
  --no-run` warning-free for touched crates; `cargo clippy --workspace --all-targets` clean —
  now *including* the core cast gate of §7 (its allows land in the same commits as the gate so
  the workspace clippy gate never goes red mid-branch); `module_budgets` untouched
  (`block_tree.rs` is not a budgeted hub; net production-line delta ≈ +15). PTY smoke suite
  mandatory-run / advisory-pass, summary quoted verbatim in the pre-merge report. No
  `cargo fmt`; all edits hand-matched to neighbors.

---

## History

- 2026-07-10 — drafted (Fable, H7 authoring thread) for Codex spec review.
- 2026-07-10 — rev 2 after Codex NOT-READY (two shell findings folded, core proof untouched):
  §4.3a rewritten — the `build_multi_replace` panic surface is `ChangeSet::from_ops`'s release
  assert on any malformed list, not just the empty-slice unwrap; fix is now a single up-front
  well-formedness guard (non-empty ∧ each `from ≤ to` ∧ ascending non-overlapping ∧ in-bounds)
  degrading to the identity no-op, superseding the incorrect "ChangeSet was already order-safe"
  claim. §1.2/§1.3/§1.4 updated (malformed ⊇ empty; full production inventory — seven call
  expressions across four caller buckets: `scratch.rs`, `blocks_marked.rs`,
  `search_ui.rs [search_replace_all]`+`[search_step_rest]`).
  §5/§6.1/§9 aligned (the reversed-pair test now asserts the identity-no-op path, not `from_ops`).
- 2026-07-10 — Codex spec-review round 2: **READY**. Guard verified complete (every op subtraction
  provably non-negative under the predicate; `from_ops` `retain+delete==len_before` satisfied;
  identity no-op valid). One Minor wording fix folded (controller-applied, self-verified by grep):
  §1.4 said "four production call sites" → seven call expressions across four caller buckets, and
  `scratch.rs [replace_marked_block]` corrected to `[move_block_to_scratch]`. No substantive change.
