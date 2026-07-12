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

## M9 — Optional: upgrade/patch pulldown-cmark
<!-- item: M9 -->

M4-rest only ISOLATES its parse panic; a real upgrade is optional, low priority.
