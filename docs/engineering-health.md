# Engineering-health notes — architecture, build, and debt

**Origin:** 2026-07-07 architecture eval (whole-app assessment, evidence-gathered from the real
tree). This is durable triage of *engineering-health* concerns — module size, build/dependency
surface, and known correctness posture — distinct from feature/UX work (`ux-backlog.md`) and the
pre-Effort-P hardening list (`CLAUDE.md`). Each item graduates to the standard gated pipeline when
picked up. Metrics below are as of `c48256f` and will drift.

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

## H1 — Decompose the two god-objects (`app.rs`, `render.rs`) · `needs-design`

`app.rs` is **5,379 lines** and `render.rs` **3,393** — past the point where one person holds them
in context, and `app.rs` mixes the reduce loop, startup/session-resume, and many concerns at once.
This is the clearest structural debt, and it bites hardest right before **Effort P**: `app.rs` is
where plugin/automation integration will land, so a plugin surface bolted onto a 5k-line reducer is
a comprehension and review hazard.

**Direction (sketch):** carve `app.rs` along seams that already exist — the reduce/message dispatch,
startup+recovery, and the per-overlay handlers — into focused modules with explicit interfaces.
Precedent for deliberate *non*-splits exists (the `ux-backlog.md` audit kept `block_tree.rs` whole
on purpose), so this is a targeted split, not a tree-wide reformat. TDD-safe: behavior-identical,
guarded by the existing e2e + reduce-loop tests.

**When:** before Effort P (highest leverage there). Not urgent for correctness.

## H2 — Interrogate the `burn`/`harper` dependency weight · `needs-design`

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
