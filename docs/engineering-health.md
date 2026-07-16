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

## H13 — `Editor` is a 58-field *data* god-object (field-clustering, not dispatch)
<!-- item: H13 -->

**Grounded (rust-analyzer documentSymbol + field grep, 2026-07-10; from the state-hub map).** `struct Editor`
(`wordcartel/src/editor.rs:368`) carries **75 fields** (was 58 at first grounding) — the whole app's mutable state on one struct. This is a
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

**AUDIT 2026-07-14 (ad-hoc-surface sweep) — reframe.** A dedicated census + scored sub-agent audit tested the
temptation to read H13 as "one god-object = the sum of several ad-hoc surfaces." It does **not** hold. Of the 75
fields, only ~12 are real ad-hoc debt: the `status` field (untyped messaging → **A17**) and the 11 overlay `Option<T>`
fields — but the overlays are debt on the **dispatch** axis, not the data axis. H13's note above is right that they
stay a **flat XOR set** (do not wrap them in a sub-struct); what is ad-hoc is their **routing** (`has_active_input_overlay`
+ the hand-parallel `if/else-if` re-written across render/mouse/app/registry/render_overlays), filed separately as
**H21** (OverlayId + OVERLAYS table). The remaining ~46 fields are legitimately distinct single-purpose state —
healthy, not debt; after A17 and H21 land, Editor's residual size is expected field-count, not remaining debt to chase.
- **PendingActions peel, refined:** of the `pending_*` cluster the audit found only the **4 prompt-payload** fields
  (`pending_save_as`, `pending_save_overwrite`, `pending_write_block`, `pending_export`) share a shape, and they
  already dispatch through the exhaustive `PromptAction` match in `prompts.rs`. The sole residual DRY nit is the two
  field-clear blocks that must stay in sync → collapse to one `Option<PromptPayload>` if a refactor touches this. The
  other `pending_*` are unrelated axes (async-job continuations, `quit_drain` queue, `pending_mark` chord state,
  `scratch_return` pointer) — a naming **rhyme**, not a shared abstraction. Do **not** build a general `PendingAction` enum.
- **Investigated and ruled NOT ad-hoc debt (so they are not re-triaged from raw census counts later):** the view-lens
  display toggles (`RenderMode`/`toggle_focus`/`measure`/scrollbar/status-line/menu-bar) — already seamed by the
  command-surface-contract's law-6 shared setter, genuine distinct side effects, a naming rhyme not duplication
  (the pattern to **imitate**, cf. E8); and the high-write census buckets (`mode`=78, `theme`=34, `scroll`=27, …) —
  each a typed enum or an already-seamed subsystem (`diag_provider.rs`'s `ProviderSet` is itself a model to imitate).
  No hidden fifth surface exists.

## Newly-tracked items (stubs)

*(Auto-created during the backlog-manifest migration. Status/size/kind live in `backlog.toml`; flesh out the triage prose here when the item is picked up.)*

## M9 — Optional: upgrade/patch pulldown-cmark
<!-- item: M9 -->

M4-rest only ISOLATES its parse panic; a real upgrade is optional, low priority.

## H22 — Universal edit chokepoint (route all internal edits through submit_transaction)
<!-- item: H22 -->

**Surfaced by A17's read-only-guard work (2026-07-15).** A truly universal single function that ALL internal
edits route through — including the direct `Buffer::apply` calls in `search_ui`/`jobs_apply`/`transform` — i.e.
making **`submit_transaction`** (the M2 untrusted-edit boundary) the one funnel everything uses. That's
genuinely valuable: it would localize versioning, swap-scheduling, reconcile-triggering, the read-only guard,
**and** the Effort-P plugin-edit seam in one place, and it rhymes with the "unify ad-hoc surfaces" arc
(A17/H21). But it is a real mutation-architecture refactor and deserves its own effort, **not** a
mid-A17 expansion.

**Why it came up:** A17 needed to make the `view_messages` history buffer non-editable, and the read-only
guard kept surfacing new mutation paths across six plan-gate rounds — because this codebase has **no single
mutation chokepoint**. The mutation surface decomposes into two closed categories: (1) *content* mutation of
an existing buffer, already closed at `Buffer::{apply, undo, redo}` (the private `document.buffer` is written
only by those three); and (2) whole-*buffer* replacement, at ~3 `Editor`-slot sites (`save::reload_from_disk`,
`save::load_recovered`, `session_restore::open_into_current`) that swap the entire `Buffer` and so bypass the
content methods. A17 handles both *locally* — it guards the closed content set and adds a small
`Editor::replace_buffer` chokepoint for the replacement sites — which is complete for A17's needs. H22 is the
larger move: collapse the *internal direct-`apply` callers* (which currently bypass `submit_transaction`, the
M2 boundary) onto one funnel too, so that versioning/swap/reconcile/read-only/plugin-edit bookkeeping all live
at a single seam rather than being re-implemented per call site. Likely **L**; a natural companion to the
A17/H21 unification arc and to any Effort-P edit-surface hardening.
