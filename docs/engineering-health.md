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

## H1 — Decompose the two god-objects (`app.rs`, `render.rs`) · **PARTIALLY SHIPPED** 2026-07-09 (merge 304e263) — `render()` body split remains

**SHIPPED 2026-07-09** (merge `304e263`, branch effort-h1-god-object-decomposition, 12-task subagent-driven
execution). The hub SEAM refactor landed behavior-identically: `run`'s 8-deadline loop → the `timers.rs`
**static fn-pointer table** (`SUBSYSTEMS` + `next_wake`/`on_tick`/`pre_recv`; gates + fire-order preserved;
idle-blocks/no-spin **proven by a compiled Fable probe** holding None at +24h); `reduce`'s ~900-line match →
a **10-stage `Handled`-protocol skeleton** + `fold_and_continue`, plus `Input(Key)` → `input::handle_key`;
the leaf extractions (`theme_cmds.rs`, `chrome.rs`, `chrome_geom.rs`, `render_status.rs`, + the micro-leaves
to their domain modules) and `list_window::apply_list_nav`. New guardrail pins assert the idle-blocks
invariant + the version-hook asymmetry. Both final gates GO (Fable whole-branch + Codex pre-merge); 1,267
tests green, clippy clean, smoke 8/8. Process: Fable authored the spec+plan (Codex-gated each round);
see [[wordcartel-fable-authors-codex-gates]].

**REMAINING — the one deferred piece, its own next effort: split the 522-line `render()` body** by paint
surface (row loop → `paint_rows`, status → `paint_status`, cursor → `place_cursor`; unify the twin
`segs`/`placed` span-builders). Deliberately scoped OUT of the shipped effort (which did verbatim render-*helper*
moves only): it is a different risk class — real restructuring that churns the pixel-exact golden-render tests —
so it earns a focused pass of its own. `render.rs` is not fully decomposed until this lands; low context
overlap with the app.rs work is exactly why it was split off (user decision 2026-07-09).

**Line anchors (2026-07-09 map; may drift — `render()` body is `render.rs:216–737`, guarded by the
`#[allow(clippy::too_many_lines)]` at :215).** The body is 12 sequential phases; the three split targets and the
dedup target:
- **`paint_rows`** ← the row loop `render.rs:358–606` (the mass). Per visual row it builds spans through two
  near-duplicate paths: the **segs** path (:395–450, no search/diag/sel/block) and the **placed** path (:451–588,
  per-glyph MarkedBlock→Selection→Search→Diag layering + run-accumulation).
- **`paint_status`** ← the status-line block `render.rs:635–699` (search bar / minibuffer / prompt / normal +
  right-flushed Ln/Col·words via `render_status::` helpers).
- **`place_cursor`** ← the hardware-cursor block `render.rs:704–734` (search field / minibuffer / `nav::screen_pos`).
- **Unify `segs`/`placed`:** the **prefix lead-in is near-verbatim duplicated** — segs at `render.rs:404–432`,
  placed at `:477–503` (same heading-numeral-box-vs-dim-glyph logic, copy-pasted), and both share the identical
  `row_dim`/`plain_source` compose ladder (:434–446 vs :515–527). That shared lead-in + style ladder is the
  natural extract. Everything after the loop is already delegated: `ChromeStyles::build` (:612, shared with
  `render_overlays.rs`), scrollbar (:617–630), and `render_overlays::paint` (:736).

*(Original triage below retained for history — the SEAM direction it sketched is what shipped.)*

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

## H4 — Arch package should declare pandoc (+ a TeX engine) as optdepends · `SHIPPED` 2026-07-08 (polish batch, a0912df: `pandoc-cli` + `texlive-xetex` optdepends)

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

## H6 — Decide a point-release version scheme + release process · **SHIPPED** 2026-07-09 (merge 50b449a, tag v0.1.0)

**SHIPPED 2026-07-09** (branch release-v0.1.0-versioning, merge `50b449a`; design doc
`docs/superpowers/specs/2026-07-09-point-release-versioning-design.md`). Resolved all four forks below:
(a) **scheme** = SemVer pre-1.0 `0.MINOR.PATCH`, `1.0.0` reserved for the Effort-P capstone (MINOR = features,
PATCH = fixes-only); (b) **source of truth** = Cargo `[workspace.package] version` (both crates inherit via
`version.workspace = true`), a git tag `vX.Y.Z` mirrors it; (c) **PKGBUILD** `pkgver()` is now tag-anchored
(`git describe --tags | sed …` → `0.1.0` at the tag, `0.1.0.rN.gHASH` between); (d) **ritual** = a hand-curated
`CHANGELOG.md` (Keep a Changelog) + a 5-step release checklist in the design doc. Also closed the "app can't
report its version" gap: new `wcartel --version` / `-V` reads `env!("CARGO_PKG_VERSION")`. First release **v0.1.0**
cut against the current tree (annotated tag on `50b449a`) and the `release-dist` Arch package built
(`wordcartel-0.1.0-1-x86_64.pkg.tar.zst`). Gates green (build/clippy/`cargo test --workspace` all suites);
`--version` verified. Advances **H4** (packaging). *(Original triage below retained for history.)*

**Question (user):** decide on a point-release and versioning SYSTEM for the app.

**Grounded (may drift):** there is NO semantic version today — the Cargo crates are `version = "0.0.0"` and
the Arch `PKGBUILD` uses a VCS-style `pkgver()` = `0.0.0.r<commits>.g<hash>` (`packaging/arch/PKGBUILD:37`),
so every build is a git-describe snapshot with no human-meaningful release points. A point-release system
means choosing: (a) a scheme (SemVer `0.x`→`1.0` aligned to the Effort-P 1.0 capstone? CalVer?); (b) the
canonical home for the version (Cargo workspace `version`, a `VERSION` file, or git tags); (c) how the
PKGBUILD `pkgver()` derives from it (tag-based `git describe` instead of a raw commit count); (d) a
tag/changelog/release ritual. Ties to the 1.0 framing (Effort P = the 1.0 capstone per CLAUDE.md) and the
H4 packaging work. Anchors: `Cargo.toml` (`version`), `packaging/arch/PKGBUILD` (`pkgver()` :37), git tags
(none today).

## H7 — Audit `.unwrap()` usage across the tree · `needs-design` (user-reported 2026-07-09)

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

## H8 — Dead public API: two fold/outline accessors have no production callers · **SHIPPED** 2026-07-09

**SHIPPED 2026-07-09** (branch chore-h8-remove-dead-accessors). Fable scoped it (compile-verified a scratch
removal against the branch) and both accessors were deleted with their exclusive tests: `outline::section_range`
(+ tests `section_range_stops_at_same_or_higher_level`, `section_range_last_heading_runs_to_eof`) and
`fold::FoldState::hidden_byte_ranges` (+ test `hidden_byte_ranges_cover_body_not_heading`). Fable also caught two
live stale doc-comment references the grep pass missed (`outline.rs` `ordered` and `sections` doc comments named
the removed fns) — both fixed. Gates green: build + clippy `--workspace --all-targets` clean; `cargo test`
`wordcartel-core`/`wordcartel` all suites pass (core 279, shell 939, oracle 42, 0 failed). Low-risk, mechanical
as predicted; no shared test helpers over-deleted (`ordered`, `DOC`, `parse`/`doc` retained). *(Original triage
below retained for history.)*

**Grounded (rust-analyzer call-hierarchy + `findReferences` + raw grep, 2026-07-09).** Two `pub` fns are
referenced ONLY by their own unit tests — superseded-but-not-removed API. Both are the byte-space /
single-shot sibling of a batch API that the real hot path uses instead:

1. **`outline::section_range`** (`wordcartel-core/src/outline.rs:75`) — referenced only inside its own file:
   the def plus 4 unit-test call sites (lines 195, 198, 201, 209). Its doc comment describes the fold
   subsystem as the caller ("callers hide the body… and keep the heading visible"), but folding actually
   uses the `sections`/`body_range` batch API — `section_range` looks like a leftover from before that batch
   API landed. Tests to remove with it: `section_range_stops_at_same_or_higher_level`,
   `section_range_last_heading_runs_to_eof` (`outline.rs:187`/`:204`).

2. **`fold::FoldState::hidden_byte_ranges`** (`wordcartel/src/fold.rs:113`) — the BYTES-space hidden-range
   accessor; grep finds only the def and one test (`fold.rs:395`, `hidden_byte_ranges_cover_body_not_heading`).
   The per-frame path uses `FoldView::compute` (LINE space, merged + `epoch`-cached via
   `editor.active_fold_view()`) instead, so the byte-space variant is never invoked in production. Same
   superseded-sibling shape as (1).

**Direction when picked up:** for each, confirm no Effort-P/plugin surface is expected to want it, then delete
the fn + its test(s) — or, if it's meant to be kept as public API for plugins, add a production caller or a
`#[doc]`/rationale so it isn't mistaken for dead code. Low-risk, mechanical. Anchors: `outline.rs:75` (+`:187`/
`:204` tests); `fold.rs:113` (+`:395` test); the batch APIs that actually feed the hot path
(`outline::sections`/`body_range`, `fold::FoldView::compute`).

## H9 — Lift the logical-line helpers out of `derive` into their own module · `triage` (2026-07-09)

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

## H10 — `reduce`'s 10-stage interception chain is verbatim boilerplate · `watch` (2026-07-09)

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

## H11 — `commands::run` is the next god-*function* after `render()` (inline bodies masquerading as a table) · `triage` (2026-07-09)

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

## H12 — PTY smoke suite has no live-splash coverage (S9) · `triage` (2026-07-10)

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

## H13 — `Editor` is a 58-field *data* god-object (field-clustering, not dispatch) · `watch` (2026-07-10)

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
