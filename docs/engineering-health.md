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

## H1 — Decompose the two god-objects (`app.rs`, `render.rs`) · `needs-design` · deferred (see "When")

`app.rs` is **5,519 lines** and `render.rs` **3,393** — past the point where one person holds them in
context. This is the clearest structural debt, and it bites hardest right before **Effort P**: `app.rs`
is where plugin/automation wiring lands, so a plugin surface bolted onto a 5k-line reducer is a
comprehension and review hazard.

**Real surface (2026-07-08 measurement — most of the line count is co-located tests):**
- `app.rs`: **~1,946 production** lines (~3,573 tests). The mass is two hubs — `reduce` (~900 lines) and
  `run` (the event loop, ~430) — plus ~25 small helpers.
- `render.rs`: **~1,028 production** (~2,365 tests), 26 paint fns. → **~3,000 production lines total**, not ~9k.

**Why it regrew (this drives the design).** The prior H1 pass (2026-07-04/05, commits `4e12212`…
`5c908f3`, all "verbatim move") extracted cohesive *leaves* — `jobs_apply.rs`, `session_restore.rs`,
`prompts.rs`, `search_ui.rs` — but deliberately left the two **hubs** (`reduce`, `run`) behind. Those
are exactly what every new interactive feature must touch, so app.rs grew **+814 lines in ~3 days**
(4,705 → 5,519) as D1/A5, E3/E4, the scrollbar/menu/status/mouse chrome, C3, R1, and the swap fix each
landed the same three shapes into the hubs: (1) a new `Msg` variant → a new `reduce` match arm; (2) a
new timed feature → a new `*_deadline` term in `run`'s loop (now **8** deadlines) + a `recompute_*`/
`*_tick` helper; (3) co-located tests. Not sloppiness — structural: the wiring belongs at the hub, and
the hub was left monolithic. **Effort P will do the same (plugin message-arms + hooks).**

**Direction — the durable fix is a SEAM, not more leaf extraction** (leaves alone regrew once already):
- **`run`'s deadline loop → a registry of timed subsystems** — each contributes its own deadline + tick,
  so a new timed feature registers a subsystem instead of editing the loop. Must preserve fire-order
  (dwell/grace first), the per-subsystem in-flight/pending gating, and the never-spin / idle-is-free
  invariant. Keep subsystems as free fns over `&mut Editor` (as today) to avoid borrow-checker friction.
- **`reduce`'s ~900-line match → per-domain handler modules** — keep the skeleton (prologue capture →
  modal/minibuffer/overlay interception chain → dispatch → epilogue: version-change hook + drain-fold);
  lift out the per-`Msg` handler bodies. The interception layering + shared epilogue are the careful
  part (ordering bugs here shift behavior).
- Finish the remaining **leaf extractions** (theme-cmds, chrome-`recompute_*`, session-persist, overlay
  dispatch) and split **render.rs** by paint surface — these are the easy, verbatim-move tasks.

**Difficulty: focused Medium.** Leaf extraction is trivial/low-risk. The two hubs are the real work —
`reduce`'s interception layering (harder) and `run`'s deadline-registry (invariant + borrow care).
Correctness risk is **low** (behavior-identical; caught by compiler + ~925 shell tests + the e2e
`reduce→advance→render` journeys + PTY smoke) — the cost is iteration-to-green, not debugging. The one
genuine risk is a **subtle emergent regression in the hub that no test covers** (exactly the swap-thrash
class), so it needs: a whole-branch review gate AND a new guardrail asserting the refactored `run` loop
still **blocks when idle** (the resource-behavior invariant).

**When (decision 2026-07-08): DEFERRED until Fable credits are back.** A hub refactor of the dispatch/
event loop is precisely the case Fable's executable whole-branch probes are worth spending on (the
subtle-emergent-behavior risk above); the user chose not to attempt it until then. Still gated **before
Effort P**. Not urgent for correctness.

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

## H4 — Arch package should declare pandoc (+ a TeX engine) as optdepends · `needs-design` (user-reported 2026-07-08)

The Arch `PKGBUILD` (`packaging/arch/PKGBUILD`) lists optdepends for clipboard (wayland/libxcb/libx11/
wl-clipboard/xclip) but **not pandoc**, even though export shells out to it: `wordcartel/src/export.rs`
runs pandoc for html/docx/pdf export. It is genuinely *optional* — `probe_pandoc()` is cached and
returns false when pandoc is absent, and callers gate on it and show a status instead of failing — so
the right declaration is an **optdepend**, not a hard `depends`: `pandoc: markdown export (html/docx/pdf)`.
The **PDF** path additionally needs a TeX engine — the pandoc `--pdf-engine` defaults to xelatex
(`config.rs:139`) — so a second optdepend is likely warranted (e.g. `texlive-xetex: PDF export via
pandoc --pdf-engine=xelatex`). Direction: add both to the PKGBUILD optdepends when next touched; confirm
the exact Arch package names for the TeX engine. Anchors: `packaging/arch/PKGBUILD`,
`wordcartel/src/export.rs`, `wordcartel/src/config.rs:139`.

## H5 — App-managed cleanup of swap files / state-dir debris? · `needs-design` (user-reported 2026-07-08)

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
