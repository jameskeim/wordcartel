# Engineering-health notes — architecture, build & debt (OPEN items)

**How this file works (backlog manifest system, since 2026-07-10):** triage prose for **OPEN**
engineering-health items — module size, build/dependency surface, correctness posture — each keyed to
`backlog.toml` by a `<!-- item: ID -->` marker. **Status/size live ONLY in `backlog.toml`; read the
generated `BACKLOG.md` for status.** Completed items' prose is in
[`backlog-archive.md`](backlog-archive.md); feature/UX items in [`ux-backlog.md`](ux-backlog.md).
Metrics cited below are point-in-time and will drift.

## Snapshot (the eval's evidence)

Measured, not impressionistic:
- **Split holds.** `wordcartel-core` 9.2k LOC (pure, `#![forbid(unsafe_code)]`, zero IO);
  `wordcartel` shell 35k. **0 unsafe blocks** in the whole tree.
- **Failure discipline.** ~35 real-runtime `.unwrap()`s across ~18k production LOC (raw grep of 740
  is ~96% co-located tests + the e2e harness); typed error enums to the status line; `catch_unwind`
  isolation for workers/parse.
- **Test investment.** 1,205 `#[test]` fns; ≈1.4 test-LOC per production-LOC; property tests +
  cargo-fuzz oracle; in-process e2e journeys + live-PTY smoke; invariants enforced as merge gates.
- **Runtime is lean, build graph is not.** Shipped binary links only glibc/libgcc/libm; but the
  lockfile is **672 crates**, including a full ML framework (`burn`/`cubecl`/tensor stack) pulled
  transitively for grammar (`harper-*`).

The verdict was: well-constructed and efficient where it counts (top-decile discipline for a solo
pre-1.0 Rust TUI); the honest caveats are the two items below. The engineering culture — process
that catches real bugs — is the real asset.

---

## H2 — Interrogate the `burn`/`harper` dependency weight
<!-- item: H2 -->

**672 crates** in the lockfile, a large share from `harper-*` grammar checking dragging in `burn`
(a tensor/ML framework) + `cubecl`. The runtime binary is unaffected (statically linked, glibc-only
`ldd`), so this is a **build-time + supply-chain** concern, not a runtime one: multi-minute cold
builds, a large audit surface, and a lot to trust — which matters more once **Effort P** opens a
plugin attack surface.

**Question to answer (not yet a decision):** does grammar quality justify a tensor runtime as a
transitive dep? Options to weigh when picked up — (a) keep it (grammar is a headline feature);
(b) feature-gate grammar so a lean build drops the ML stack; (c) a lighter POS/grammar backend.
This is the biggest efficiency lever *not* yet pulled; needs a real look at what `harper-brill`
actually requires from `burn`.

**When:** opportunistic; pairs naturally with the pre-Effort-P dependency/audit pass.

## H3 — Incremental-parser tail divergences · **NOT open correctness debt (corrected)**
<!-- item: H3 -->

The eval initially flagged the `incremental ≡ full` divergences as "the one open correctness item."
**Checked against our notes (2026-07-07) — that framing is wrong.** The accurate status:

- **Cosmetic, never data-loss/panic.** The F2 fuzz oracle still finds tail divergences (nested
  containers, loose/tight lists) where the incremental tree disagrees with a full reparse; the
  effect is wrong styling/conceal/fold/outline for at most a moment — not corruption, not a crash.
- **Self-healing, and cannot accumulate.** `effort-incremental-reconcile` shipped (merged
  `1c97cda`; `wordcartel/src/reconcile.rs`) with a **convergence theorem**: after ~150 ms without
  edits (`RECONCILE_DEBOUNCE_MS`), a version-checked background full reparse lands and
  `document.blocks` becomes *exactly* `full_parse(text)`. Each reconcile re-bases the incremental
  engine on a known-correct tree, so divergence can't build up across edits.
- **Perfect closure was a reasoned non-goal.** The 2026-07-01 incremental-soundness spec chose
  **Option A (eventual consistency)** over **Option B (model loose/tight + nested-container
  extents)** deliberately: B is an open-ended modeling investment that adds complexity and pushes
  more edits onto O(document) fallbacks (a responsiveness regression) — "for correctness on
  adversarial inputs a user rarely types." Structurally, loose↔tight is a *non-local* effect (one
  blank line in a list restyles every item), so localized detection **cannot win the tail**.

**Conclusion:** there is no cheap path forward *and no need for one*. This is a known, bounded,
deprioritized-by-design item — not a risk. It stays on `CLAUDE.md`'s "none blocking; sequence by
yield" list only as a **yield** item (chase the tail further only if a real user-visible case
appears, or as a side effect of a future B-style investment). It should **not** be re-raised as
open correctness debt. `block_tree.rs` remains the shared hotspot for both this and the R1
paragraph-end widen cost, so any future work there touches both.

## H5 — App-managed cleanup of swap files / state-dir debris?
<!-- item: H5 -->

**Question (user):** should there be an in-app way to clean up swap files and other filesystem debris,
or is that something the user does outside the program?

**Grounded (may drift):** the app writes crash-recovery + session state under the XDG state dir
(`~/.local/state/wordcartel`, `swap::state_dir`): per-doc `*.swp` (hashed path), scratch
`scratch-{pid}.swp`, `session.toml`, and — as the swap durability work surfaced — occasional orphaned
atomic-write `*.tmp` files and stale swaps (e.g. the intentionally-left stale swap after a SaveAs
rekey, or scratch swaps from crashed sessions). `swap::find_orphan_scratch_swap` already scans for
crashed-scratch orphans on launch (for *recovery*), but nothing *prunes* accumulated debris. Forks to
weigh: (a) auto-prune on launch (delete swaps whose owning pid is dead AND whose doc is clean/unchanged);
(b) an explicit command (`Clean recovery files…`); (c) leave it to the user + document the dir. Ties to
the swap durability model (memory: `wordcartel-swap-idle-thrash`). Anchors: `wordcartel/src/swap.rs`
(`state_dir`, `swap_path`, `find_orphan_scratch_swap`), `recovery.rs`.

## H7 — Panic-safety & arithmetic-soundness audit
<!-- item: H7 -->

**Scope broadened 2026-07-10** from ".unwrap() only" to all runtime panic sites — `.unwrap()`
(772 raw, ~35 real-runtime), `.expect()` (161 — ensure each carries an invariant message), `panic!`
(44 raw), `unreachable!` (4) — **plus an arithmetic-soundness sweep** of the ~147 numeric casts
(`as usize`/`u32`…) on `BytePos`/offset math for overflow/truncation (a Codex-flagged class). Same
shape/sequencing as the original unwrap audit; the `.unwrap()` classification below is its core.

**Sequencing (`depends_on = [H11, H14]`, 2026-07-10):** do this AFTER the `commands::run` (H11) and
`render()` (H14) decompositions — those rewrite exactly the edit-/paint-path bodies where the ~35
unwraps concentrate, so auditing first means re-auditing after the churn. Land it BEFORE Effort P
(panic-safety hardening ahead of the untrusted plugin surface). Otherwise low-risk/mechanical — a good
palate-cleanser between the heavier decomposition efforts.

**Question (user):** audit and consider `.unwrap()` usage.

**Grounded (may drift):** the policy already exists — "no `.unwrap()` on fallible/external paths; a guarded
unwrap immediately after establishing the invariant is acceptable, but prefer `.expect(\"…invariant…\")`"
(CLAUDE.md Rust conventions) — and the 2026-07-07 eval measured ~35 real-runtime `.unwrap()`s across ~18k
production LOC (the raw grep of ~740 is ~96% co-located tests + the e2e harness; see this doc's Snapshot).
This item = a deliberate sweep of those ~35: classify each as guarded-invariant (→ convert to `.expect` with
a message) vs genuinely-fallible (→ typed error to the status line), and confirm none sit on an
untrusted/IO/async path that M2/M3/M4 didn't already cover. Low-risk, mechanical, high-confidence — a good
fold into a future hardening pass. Anchors: CLAUDE.md (unwrap policy), this doc's Snapshot (the ~35 count),
the M2/M3/M4 boundaries.

## H9 — Lift the logical-line helpers out of `derive` into their own module
<!-- item: H9 -->

**Grounded (rust-analyzer call-hierarchy + raw grep, 2026-07-09):** `derive::{total_logical_lines, line_start,
line_text}` (and the render-mode mapper `line_render_for`) are pure logical-line/line-space utilities with no
dependence on the derive PIPELINE — yet they live in `derive.rs` (the 979-line recompute module) and are the
most cross-imported thing in it. `line_start` alone has ~30 call sites across `nav.rs` (heavily), `render.rs`,
and `transform.rs`; `total_logical_lines` is used from `nav.rs`/`prompts.rs`/`commands.rs`; `line_text`/
`line_render_for` from `nav.rs`. So the whole nav/render line-space layer imports `derive` only to reach three
trivial helpers, coupling it to the parse/layout hub. Direction when picked up: lift the trio (plus
`line_render_for`) into a small `lines.rs` (or a `buffer`-adjacent home), leaving `derive` to own only the
`rebuild`/`rebuild_downstream` pipeline + `LayoutKey`. Mechanical (move + re-point imports; no behavior change),
low-risk, and a natural seam to take **alongside the remaining H1 `render()`/module-size work** rather than as a
standalone churn. Anchors: `wordcartel/src/derive.rs:91` (`total_logical_lines`), `:104` (`line_start`), `:116`
(`line_text`), `:25` (`line_render_for`); heaviest consumer `nav.rs`.

## H10 — `reduce`'s 10-stage interception chain is verbatim boilerplate
<!-- item: H10 -->

**Grounded (read of `app.rs:233–252`, 2026-07-09).** After the H1 SEAM refactor, `reduce` opens with a
10-stage overlay/modal interception chain — `marks → menu → palette → theme_picker → file_browser → prompts →
minibuffer → search_ui → diag_overlay → outline_overlay` — where every stage is the identical line
`let msg = match crate::X::intercept(msg, …) { Handled::Done(k) => return k, Handled::Pass(m) => m };`, differing
only in the module path and arg list. It is cohesive, blessed-style *flat dispatch* (the house rules explicitly
allow a long flat dispatch), so this is **NOT** a defect today — filed only as a `watch` item. The reason it
can't already be a clean fn-pointer table / `SUBSYSTEMS`-style row set: two stages (`menu`, `palette`) need
`reg` + `keymap` in their `intercept` signature while the other eight take only `(msg, editor, ex, clock,
msg_tx)`, so a uniform table would need a widened shared signature or a two-tier split. **Trigger to act:**
Effort P adds plugin-contributed intercept stages here — the moment the chain grows past its current fixed set,
collapse it (an `intercept_chain!` macro, or unify the stage signature so the chain becomes a slice of handler
fns iterated in order). Until then the repetition is bounded and readable; touching it now is churn for its own
sake. This is a **command-surface-contract / Effort-P-conformance** note, not module-size debt (`reduce` is
within budget). Anchor: `wordcartel/src/app.rs:233–252` (the chain); `:123` (`Handled`); the `timers::SUBSYSTEMS`
table is the model a unified version would follow.

## H11 — `commands::run` is the next god-*function* after `render()` (inline bodies masquerading as a table)
<!-- item: H11 -->

**Grounded (read of `commands.rs:209–687`, 2026-07-09).** `commands::run` — the `Command`-enum dispatcher every
built-in and every registry handler ultimately routes through — is **478 production lines** carrying
`#[allow(clippy::too_many_lines)] // command dispatch — a flat table, one arm per Command variant`. But unlike the
genuine tables (`registry::builtins` = pure data rows; `reduce` = thin delegations; `timers::SUBSYSTEMS` = a
fn-pointer table), its arms are **inline edit bodies**, not delegations — so it violates the letter of the
*"a match arm is a thin delegation into a domain module — never an inline body"* anti-regrowth rule (CLAUDE.md →
Module structure) while claiming the flat-table exception. This is the same god-*function* class as the remaining
H1 `render()` split, and it's on the **Effort-P hot path** (plugin-invoked edits route through `run`), so it is
worth decomposing before P.

**Two low-risk moves, both preserving the flat-dispatch shape:**
1. **Factor the repeated edit epilogue.** Every edit arm ends with the verbatim
   `derive::rebuild(editor); nav::ensure_visible(editor); editor.active_mut().desired_col = None; CommandResult::Handled`
   (across `InsertChar`/`InsertNewline`/`Backspace`/`DeleteForward`/… ~8+ arms), and each first branches
   selection-vs-collapsed identically. One helper (e.g. `apply_edit_and_settle(editor, txn, edit, kind, sel, clock)`)
   collapses both — exactly the move `app::fold_and_continue` made for `reduce`'s "21 verbatim repetitions."
2. **Lift the edit-arm bodies into an `edit` module** (insert/delete/replace-selection primitives over the
   `ChangeSet`/`Edit`/`Transaction` builders already in this file — `replace_changeset`, `build_range_replace`,
   `build_multi_replace`), leaving `run` a thin exhaustive dispatch like `reduce`.

**Difficulty: focused Medium.** Behavior-identical; caught by the ~55 `commands::run` unit tests + the e2e
journeys. Lower risk than the H1 `render()` split (no pixel-exact golden churn — the edit paths are asserted by
buffer state, not rendered cells). Pairs naturally with the H1 module-size pass. Anchors: `wordcartel/src/commands.rs:209`
(`run` + the allow at `:210`), the changeset builders at `:101`/`:128`/`:156`, the repeated epilogue visible at
`:224–226`/`:238–240`/`:271–273`/`:287–289`.

## H12 — PTY smoke suite has no live-splash coverage (S9)
<!-- item: H12 -->

**Grounded (2026-07-10, splash effort merge 242c987).** The startup splash covers the first frame on every
launch and would fail all 8 PTY smoke first-frame checks, so `scripts/smoke/tmux-drive.sh`'s `start_wcartel`
now passes `--no-splash` on EVERY launch (alongside `--no-config`). Necessary — the smoke checks assert on
first-frame content and the splash is not what they test — but the side effect is that **no smoke check ever
exercises the real splash or its dismissal** (the Fable whole-branch review flagged this as advisory M-C). The
splash IS covered by in-process e2e journeys (`wordcartel/src/e2e.rs`: show-on-first-frame, key/mouse dismiss,
`--no-splash`, recovery-suppression at render level) and unit tests, so this is a *live-binary* coverage gap,
not a correctness gap. Direction when picked up: add an **S9** check that launches WITHOUT `--no-splash` and
asserts the real journey — wordmark/tagline on the first frame → a key dismisses it → the editor (or the opened
file) is revealed — plus optionally a swap-recovery-relaunch variant asserting the recovery prompt wins over
the splash on the live binary (the in-process e2e + the controller's manual PTY repro already proved this;
S9 would lock it into the mandatory-run suite). Low-risk, additive (one new check script + the launcher already
supports per-launch args). Anchors: `scripts/smoke/tmux-drive.sh` (`start_wcartel`, the `--no-splash` default),
`scripts/smoke/checks/`, `wordcartel/src/splash.rs`, the e2e journeys in `wordcartel/src/e2e.rs`.

## H13 — `Editor` is a 58-field *data* god-object (field-clustering, not dispatch)
<!-- item: H13 -->

**Grounded (rust-analyzer documentSymbol + field grep, 2026-07-10; from the state-hub map).** `struct Editor`
(`wordcartel/src/editor.rs:368`) carries **58 fields** — the whole app's mutable state on one struct. This is a
*data* god-object, NOT the *dispatch* god-object class H1 targets: there is no growing `match`/loop here (editor.rs
is struct definitions + accessors/small mutators), so it is **outside the anti-regrowth GATEs** (`too_many_lines`,
`module_budgets`) and is **not a defect** — filed as `watch`, not `triage`. The field count is genuine app-state
breadth, and several cohesive clusters are ALREADY peeled into sub-structs (`MouseState` — the dwell/reveal timers;
`Document`/`View`; `QuitDrain`/`PendingAfterSave`), which is the right instinct. Two clusters remain inline that
would be the natural next sub-structs IF this ever wants tidying:
- **save/quit lifecycle** — `pending_after_save`, `pending_save_as`, `pending_save_overwrite`, `pending_write_block`,
  `pending_export`, `pending_mark` (`editor.rs:383–394`) — 6 fields modelling the "do X after the save completes"
  state machine; could become a `PendingActions` sub-struct.
- **clipboard** — `clipboard_sync_request`, `clipboard_get_pending`, `clipboard_notice_shown`, `clipboard_provider`,
  `clipboard_provider_dirty` (`editor.rs:395–403`) — 5 fields; could become a `ClipboardState` sub-struct.
**Trigger to act:** only a related refactor (e.g. Effort P wanting a cleaner plugin-facing state surface) or if the
struct crosses a comprehension threshold — do NOT split for its own sake (over-fragmentation is its own pathology
per the Module-structure rule). Note the 10 overlay `Option<T>` fields (`prompt`/`palette`/…/`splash`) are a
deliberate flat XOR set enforced by the `open_*` family, not a clustering candidate. Anchor: `editor.rs:368` (the
struct), `:493` (the impl), the `open_*` overlay family + shared setters.

## Newly-tracked items (stubs)

*(Auto-created during the backlog-manifest migration. Status/size/kind live in `backlog.toml`; flesh out the triage prose here when the item is picked up.)*

## H14 — Split the render() body by paint surface
<!-- item: H14 -->

Split 522-line render() into paint_rows/paint_status/place_cursor; unify segs/placed lead-in. (H1 remainder.)

## M8 — M5 follow-up: undo louder-hint for buffer-level merges
<!-- item: M8 -->

Finish the louder undo-eviction hint for buffer-level merges (the last M5 follow-up).

## M9 — Optional: upgrade/patch pulldown-cmark
<!-- item: M9 -->

M4-rest only ISOLATES its parse panic; a real upgrade is optional, low priority.

## H17 — Pre-P public-API doc-coverage sweep
<!-- item: H17 -->

**Grounded (2026-07-10):** the house style requires a doc-comment on every public item, but coverage
isn't enforced — `wordcartel-core` alone exposes ~180 `pub fn/struct/enum/trait/const/type`. **Effort P
exposes this surface to plugins**, so it should be documented, and kept documented, before then.
Direction: doc-comment the undocumented public items (params/returns/errors; a runnable `# Examples`
block for non-obvious fns, per the CLAUDE.md "Docs" convention), then land `#![warn(missing_docs)]` (at
least on `wordcartel-core`) so coverage can't regress — a gate in the same spirit as the backlog drift
gate and `module_budgets`. Orthogonal to the god-object decompositions (no `depends_on`); do it anytime
before Effort P. Mechanical, low-risk. Anchors: `wordcartel-core/src` public items; CLAUDE.md "Docs".

## H18 — Supply-chain audit (cargo audit / cargo deny)
<!-- item: H18 -->

**Grounded (2026-07-10):** no `deny.toml`/audit config exists today, and the lockfile is large (672
crates — much of it the `burn`/`harper` tensor stack; see H2). Before Effort P opens an untrusted plugin
surface, run a supply-chain pass: `cargo audit` (RUSTSEC CVEs) and/or `cargo deny` (advisories + license
policy + duplicate/banned-crate checks), and decide whether to wire it as a CI gate. **Pairs with H2**
as the pre-P dependency pass, but on a distinct axis — H2 = build-time weight, H18 = vulnerabilities /
licenses. Forks: audit-only vs a full `deny` policy; gate vs advisory. Anchors: `Cargo.lock`, H2.
