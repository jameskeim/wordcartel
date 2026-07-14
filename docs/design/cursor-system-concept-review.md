# Review: "A Unified Cursor System" concept vs. the real codebase

**Reviewed:** `cursor_system_concept.md` (repo root, 176-line brainstorm)
**Date:** 2026-07-13 · **Status:** analysis only — proposes, applies nothing
**Question answered:** how much of this can actually be implemented here, and how should
it be concepted and scoped for wordcartel?

---

## 0. Executive summary

**Most of the concept is buildable, and a surprising amount already exists.** The
WordStar block-marker system the concept designs (§4) SHIPPED as Effort 9A
(`blocks_marked.rs`): one begin/end pair, byte-offset anchored, edit-mapped, with a full
command family, a Block menu, and `ctrl-k b`/`ctrl-k k` bindings. Its §4 "settled
behaviours" are ~70% implemented — what's missing is purely *visual* (the `[`/`]`
glyphs, lone-begin visibility, off-screen hints). Fork B (display-only insertion) is
feasible and lands in machinery the S6 ventilate lens just proved out (`layout()` /
`ColMap` — display already diverges from source via conceal, prefix glyphs, and the
ventilate reflow), but it is the one piece with real correctness stakes and the only
Medium-hard item. The concept's biggest factual gaps against this codebase: **the
writing caret is the hardware cursor today** (three placement arms in
`render.rs::place_cursor`; no DECSCUSR shape control anywhere — that's open backlog item
B8); **there are no split panes** (§5's background-pane cursor targets a surface that
doesn't exist); and **an app-owned blink timer collides head-on with the "idle is free"
resource law** (the run loop blocks for 3600 s when nothing is armed — a painted
blinking caret would end that, permanently).

**Recommended lean (details in §3):** invert the concept's Fork-A lean. Keep the
hardware cursor as the writing caret (B8 delivers shape + blink config cheaply, native
blink is idle-free, screen-reader tracking stays strong) and paint everything else —
which must be painted regardless. Scope as three efforts: **C1** caret discipline + B8
(cheap, high leverage), **C2** marker glyph visibility with the Fork-B decision inside
it (the only hard part), **C3** optional painted-cursor mode (the brightness/dim dream)
— deferred until the demand is proven, because brightness is the *only* benefit
exclusive to the painted path.

---

## 1. Ground truth: how cursors work in this codebase today

Established by reading the real code; every claim is anchored to a symbol.

### 1.1 The caret is the hardware cursor — and only three things place it

`wordcartel/src/render.rs::place_cursor` (render phase 12, called at `render.rs:310`)
has exactly three arms, all `frame.set_cursor_position` (ratatui 0.30 / crossterm):

1. **Search bar open** → caret on the status row at the focused field position.
2. **Minibuffer open** → caret on the status row at prompt + text position.
3. **Otherwise** → the editor caret at `nav::screen_pos(editor)` in the text area
   (D2 clamp to `tg.text_width`).

No caret is painted anywhere — no styled-cell caret exists in the codebase. No
`SetCursorStyle`/DECSCUSR is ever emitted (`term.rs` only issues `cursor::Show` on
restore paths). Terminal-native shape and blink are whatever the user's terminal
defaults to. **Backlog item B8 ("Configurable terminal caret shape / colour — emit
DECSCUSR … restore on exit/panic", needs-design) is already this concept's §3 hardware
path.**

**A real wart the concept would fix:** the palette, outline, theme-picker, and
file-browser overlays each keep a `cursor: usize` field for their query line
(`palette.rs:23` etc.) but **paint no caret at all** (`render_overlays.rs` renders the
query as a plain `Paragraph`, never calls `set_cursor_position`). Meanwhile
`place_cursor`'s third arm has no overlay gating — so while a centered modal is open,
the hardware cursor stays parked at the *editor caret's* text-area position, blinking
through/over the overlay. §5's settled rules ("modal blanks the background; the modal's
input carries the caret") are not just cohesion polish — they correct an existing defect,
and on the hardware path the fix is a few lines per overlay arm.

### 1.2 The WordStar block system already shipped (Effort 9A)

`wordcartel/src/blocks_marked.rs` + `editor.rs`:

- `MarkedBlock { start, end, hidden }` — byte offsets; `pending_block_begin:
  Option<usize>` for the lone begin-marker. **One pair at a time** (`Option`, not a
  Vec) — the concept's open item 3 is answered by the code: one pair, by construction.
- **Logical anchoring is done:** both `marked_block` and `pending_block_begin` map
  through every edit (`editor.rs:289–296` maps them via the ChangeSet; a fully-deleted
  block collapse-clears; tests `marked_block_tracks_edits_and_collapses`,
  `marked_block_boundary_inserts_stay_outside`). The concept's "persistence has a
  data-model consequence" callout is already satisfied.
- **Command family:** `block_begin` (^KB), `block_end` (^KK), `block_copy`,
  `block_move`, `block_delete`, `block_jump_begin/end`, `block_toggle_hidden`,
  `block_clear` (clears both + pending — the ^KH clear affordance),
  `block_write` (^KW), `mark_block_from_selection`, `select_marked_block` (the A11.3
  block→selection bridge). All registered in `registry.rs` under `MenuCategory::Block`;
  WordStar-style chords live in `keymap.rs:423–424`.
- **Region rendering exists:** `render.rs::row_spans_placed` composes the
  `SE::MarkedBlock` face below Selection/Search/Diag over the block's placed glyphs
  (fold-safe by construction; `hidden` suppresses it). The face has a mono-depth
  fallback (reverse+bold+underline — `theme.rs:1331` a11y test), so it expresses under
  no-color/terminal-plain themes.
- **"No pre-region fill" holds trivially** — in fact *too* trivially: a lone
  `pending_block_begin` renders **nothing at all**. The writer drops ^KB and gets only a
  status-line message; the landmark is invisible. This is the concept's strongest
  genuinely-missing piece.

Not existing: bracket glyphs, any visual for the pending begin, caret-on-marker
collision handling, off-screen marker indicators, column/rectangular mode.

### 1.3 Fork B's machinery: display ≠ source is an established, proven regime

`wordcartel-core/src/layout.rs` is *the* place where the view diverges from the bytes:

- **Conceal** already *removes* source cells from display (`LineRender::Concealed`
  drops markdown markers); `Placed { src: Range, row, col, width, text, style }` is the
  per-grapheme display cell with its source anchor.
- **Non-source columns already exist** — `prefix_glyph`/`prefix_width` (bullets, quote
  bars, heading glyphs) and the S6 ventilate gutter (`GUTTER_COLS = 6`) prepend columns
  that map to no byte; `ColMap.visual_to_source` clamps clicks in the prefix region up
  to the first text glyph (`layout.rs:115–118`).
- **The consumer discipline is already enforced:** S6 migrated every read/transition
  site (caret placement, click mapping, vertical motion, selection painting) through the
  window-aware resolvers (`ventilate::resolve`, `origin_of`,
  `layout_block_as_displayed`/`_on_demand`), and `sentence_display` demonstrates the
  byte-safety pattern (its normalization is deliberately byte-length-preserving so
  `src` offsets stay valid).
- Zero-width/wide-cell mapping policies are explicit and tested
  (`visual_to_source` two-pass policy, `layout.rs:103–131`).

So Fork B ("injected, non-document cells mid-row") is **not a new class of problem
here** — it is the third instance of an existing class (conceal removes cells; prefix
and gutter prepend cells; markers would *insert* cells). The invariants it must honor
are exactly the ones enumerated in §4 below, and they are all local to `layout()` +
`ColMap` + tests. The buffer can never be corrupted by construction: injection happens
at layout time from `marked_block`/`pending_block_begin` state; save/word-count/search
all read the buffer, not the display (`count::word_count`, search over buffer bytes) —
the concept's "nothing must be stripped on save" is automatic.

### 1.4 There are no split panes

`Editor` is `buffers: Vec<Buffer>` + `active: usize` — multiple buffers, **one view,
one rendered pane**. No split/pane machinery exists in `editor.rs`, `chrome.rs`, or the
E3/E4 chrome model (the six-face chrome ladder governs bars/overlays/canvas, not
multiple editor viewports). `SE::FocusDim` exists ("e.g. a pane losing focus") as a
reserved face, but §5's "background-pane cursor" designs behavior for a surface that
does not exist and is on no roadmap item.

### 1.5 The idle-free law and the timer substrate

`wordcartel/src/timers.rs`: every wake source is a row in the `SUBSYSTEMS` table with
its own anti-spin gate; `next_wake` folds their min; when idle every gate yields `None`
and the loop blocks on a 3600 s fallback (`app.rs:797–810`). CLAUDE.md's resource law:
background work is **edge-triggered by a real state change, never level-triggered off
wall-clock**. A painted blinking caret is a permanent ~2 Hz wall-clock wake whenever the
app runs — a direct collision (see §4.1 for the reconciliation). Notably, the concept's
own §6 "idle/away state" (blink stops after N seconds of stillness) is the exact shape
that reconciles it — an edge-armed, self-clearing timer like `scrollbar` fade.

Also relevant: **focus events are not enabled** — no `FocusGained`/`FocusLost` anywhere
in `wordcartel/src`. The painted path's focus handling is net-new plumbing (small:
crossterm supports it; the input thread forwards events already).

### 1.6 Config, command surface, and the picker precedent

- `settings.rs::SettingsSnapshot` carries LAW 2 explicitly: *every persisted field MUST
  be changeable via a registered command* — enforced by the
  `every_persisted_setting_has_a_command` invariant test. New cursor options must land
  as snapshot fields + `OverridesFile` keys + diff-law entries.
- The multi-state option pattern is established: set-per-state primitives
  (palette-only) + one stateful cycle representative with state-in-label
  (`cycle_render_mode`, `cycle_scrollbar`, `clipboard_provider_cycle` —
  `registry.rs:294–, 631–, 672–`). A cursor-shape option follows this exactly.
- **The live-preview picker already has a template:** `theme_picker.rs` — captures
  `original` on open, previews on every selection move
  (`preview_selected_theme`), restores on Esc, applies on Enter, mirrors the palette's
  list-window UI. A cursor picker is a clone of this shape. On the hardware path,
  shape and blink preview live (DECSCUSR applies immediately); only brightness is a
  dead key — the concept's Fork-A table is accurate on this point.

### 1.7 Odds and ends the concept asks about

- **Multi-cursor (§6):** the core data model is already reserved —
  `wordcartel_core::selection::Selection` is `SmallVec<[Range; 1]>` + `primary` index.
  Painting and input semantics are entirely absent. The concept's "reserve for it now"
  is satisfied at the data layer for free.
- **Find/match highlight (§6):** exists — `SE::SearchMatch`/`SE::SearchCurrent`,
  layered in `row_spans_placed` with documented precedence (base → MarkedBlock →
  Selection → Search → Diag). It is already "the same theme family."
- **Testability:** hardware-cursor placement is already asserted in tests via
  `TestBackend::cursor_position` (`render.rs:846`); painted cells are assertable
  through buffer-cell inspection — both forks are testable in-process.

---

## 2. Feasibility map

| Concept item | Verdict | Grounding |
|---|---|---|
| §2.1 Fork A, Option A (hardware + DECSCUSR) | **NET-NEW, small — already backlogged as B8** | `place_cursor` is the single seam; `term.rs`/`panicx.rs` own restore. Shape menu: block/underline/bar × steady/blink (DECSCUSR 1–6). Live preview of shape+blink works; brightness/dim impossible (concept's table is correct). |
| §2.1 Fork A, Option B (painted caret) | **FEASIBLE, moderate — with one invariant collision** | Painting = patching one cell's style post-`paint_rows` (cheap, O(1)); blink = a new `SUBSYSTEMS` row that ends idle-free unless idle-settled (§4.1); focus handling net-new (§1.5); screen-reader regression real. |
| §2.1 Fork A, Option C (hybrid, OSC 12 tint) | **DISCOURAGE** | New cross-terminal compat matrix; fights the theme-standardization stance (terminal-plain/no-color constraints are durable); concept itself rates it highest-cost. |
| §2.2 Fork B (display-only injected `[`/`]` cells) | **REUSES `layout()`/`ColMap` — feasible, Medium, the one hard item** | Third instance of an existing display≠source class (§1.3). Needs: an injections parameter to `layout()`, mapping-policy rules for zero-source-width cells, wrap accounting, and injection in *both* layout paths (per-line and `ventilate::layout_block`). Risks enumerated §4.2. |
| §3 configurable writing cursor (shape/blink) | **FEASIBLE — B8 on hardware path** | Config surface must be commands + persisted settings per LAW 2 (§1.6); set-per-state + cycle pattern exists. |
| §3 brightness/dim, theme-colored caret | **Painted path only; also depth-bounded** | Brightness interpolation toward `base_bg` needs RGB depth; under the 256/16/mono depth ladder it degrades to steps or nothing. Honest only at `Depth::Rgb`. |
| §3 live WYSIWYG picker | **ALREADY HAS A TEMPLATE** | `theme_picker.rs` preview/Esc-restore/apply pattern (§1.6). |
| §4 marker pair, anchoring, clear, persistence | **ALREADY EXISTS** | `blocks_marked.rs`, `editor.rs:289–296`, full command family + menu + chords (§1.2). |
| §4 region styling once both markers exist | **ALREADY EXISTS** | `SE::MarkedBlock` compose in `row_spans_placed` (plus a `hidden` toggle the concept doesn't have). |
| §4 visible `[`/`]` glyphs; lone-begin visibility | **NET-NEW — the real §4 delta; gated on Fork B** | `pending_block_begin` is invisible today. Two implementation options (§3.2). |
| §4 caret-on-marker alternate blink | **Painted-path-only; mostly unnecessary on hybrid** | With a hardware caret, the terminal draws the cursor *over* the marker cell — the overlap resolves natively, no timers. Out-of-phase blink is only needed if both are painted, and it is a *permanent animation* while adjacent (idle-free collision, §4.1). |
| §4 off-screen marker indicator | **NET-NEW, small-medium** | No existing gutter/edge-indicator machinery; scrollbar overlay is the nearest neighbor. Deferrable. |
| §4 column/rectangular block mode | **NET-NEW, large — DEFER** | Nothing supports it; `Selection` multi-range could model it later. Out of scope. |
| §5 background-pane cursor | **INFEASIBLE TODAY — no splits exist** (§1.4) | Reserve `SE::FocusDim` + the painted-marker subsystem as the future hook; design nothing else now. |
| §5 modal hides background cursor; modal input carries the caret | **FEASIBLE, cheap — fixes a live wart** | §1.1: overlay query fields have cursor state but no caret; the hardware caret parks in the text area under modals. Hardware-path fix is a few `place_cursor` arms. "Uses the configured writing-cursor style" is automatic on the hardware path (DECSCUSR is global). |
| §6 find/match highlight | **ALREADY EXISTS** | §1.7. |
| §6 multi-cursor | **Core data model reserved; UI DEFER** | §1.7. |
| §6 ghost/completion caret | **DEFER — no completion system exists** | Nothing to preview. |
| §6 idle/away state | **ADOPT — it's the blink reconciliation** | §1.5/§4.1: edge-armed blink that settles after N s idle is what makes any painted blink lawful. |
| §7 "one painting model, no hardware cursor survives" | **CHALLENGE** | §3.1: the codebase economics favor hardware-caret + painted-role-marks. One *theme* model, two mechanisms — which is what the concept's own accessibility fallback concedes anyway. |

**Headline:** everything in the concept except column-mode and the background-pane
cursor is implementable. Roughly: ~40% already exists, ~35% is cheap reuse/net-new-small
(B8, overlay caret discipline, picker, config surface), ~20% is one Medium effort with
correctness stakes (Fork B injection), ~5% is infeasible-today (§5 panes) or
should-not-build (OSC 12 hybrid, column mode now).

---

## 3. Recommended concept & scope

### 3.1 Resolve Fork A: hardware-primary writing caret, painted role marks (invert the concept's lean)

The concept leans Option B (painted-primary). The code leans the other way:

- The three requirements pushing toward painted are **shape, blink, brightness** —
  DECSCUSR delivers shape and blink natively, restore-on-exit included, as the
  already-filed B8. Brightness is the *only* casualty.
- Native blink is free and idle-free; painted blink requires a new timer subsystem plus
  an idle-settle design just to stay lawful (§4.1).
- Painted-primary requires focus-event plumbing (net-new) and accepts a screen-reader
  regression that the concept then patches with a hardware fallback mode — i.e., both
  systems get built anyway. Build the cheap one first and *promote* it to "fallback"
  only if the painted mode ever ships.
- Role cursors and markers must be painted regardless (the concept's own single most
  important constraint). Unification should happen at the **theme layer** (one
  `SemanticElement` family, one compose ladder — this already exists: `MarkedBlock`,
  `FocusDim`, the mono-fallback discipline), not at the mechanism layer.
- Caret-on-marker collision resolves natively on this path (terminal cursor overlays
  the cell) — the concept's most fiddly settled behavior (out-of-phase blink) becomes
  unnecessary.

The picker question (§3 of the concept) then resolves itself, as the concept predicted:
a `theme_picker.rs`-pattern modal with live preview — shape and blink morph the real
caret in place via immediate DECSCUSR; there is simply no brightness row until/unless C3
ships.

### 3.2 The Fork B decision belongs inside the marker effort — and has a cheaper sibling

Keep Fork B *provisionally* settled as display-only insertion (the model is right:
logical home = document offset — **already true in the shipped code**; visual form =
injected cell). But present the human the real cost fork before building:

- **Option B-full — injected marker cells** (the concept's settled form). True
  WordStar look: `…text [block] more…`, columns shift. Cost: extend
  `layout()` with an injections input; define the mapping policy for
  zero-source-width injected cells in both `ColMap` directions; wrap/desired-column
  accounting; inject in the per-line path AND `ventilate::layout_block` (markers inside
  a ventilated window); a real test matrix (see §4.2). Medium.
- **Option B-lite — styled boundary cells, no injection.** Paint the begin marker as a
  reversed/bracket-styled *existing* glyph at the marker offset (compose a
  `SE::BlockMarkerBegin/End` face over the cell in `row_spans_placed`, exactly like
  `MarkedBlock` today), plus a one-cell painted `[`/`]` only at positions with no glyph
  (end-of-line, empty line) where no column shift can occur. Zero layout impact, zero
  mapping risk, ships in days. Cost: the marker is a *highlighted character*, not an
  inserted bracket between characters — less WordStar-faithful, slightly ambiguous for
  zero-width positions mid-word.

B-lite delivers the actual UX gap (a lone ^KB is visible; block ends are visible) at
~15% of the cost; B-full is the aesthetic completion. A legitimate path is B-lite in C2
now, B-full later if the lens/layout work (E8) makes injection cheap in passing.

### 3.3 Effort decomposition

- **Effort C1 — caret discipline + B8 (Small-Medium; do first; no dependencies).**
  DECSCUSR shape/blink as persisted settings + commands (set-per-state primitives +
  cycle representative; LAW-2 test coverage), restore on exit AND panic
  (`term.rs`/`panicx.rs`), the cursor picker (theme-picker pattern, live preview), and
  the **overlay caret fix**: `place_cursor` gains arms for palette / outline /
  theme-picker / file-browser query fields, and never parks the caret in the text area
  while a modal is open. Delivers most of §3 + §5's settled modal rules. Conforms to
  the command-surface contract explicitly.
- **Effort C2 — marker visibility (Small if B-lite, Medium if B-full; independent of
  C1).** Lone-begin + block-boundary glyphs per §3.2's resolved fork; optionally the
  off-screen marker gutter hint. Co-design flag: if B-full, coordinate with E8 (lens
  surface) since both live in `layout()`/`ColMap`; markers-inside-ventilate must be
  specified either way. S4's object/MarkedBlock work consumes the same state but not
  the same rendering — no hard dependency.
- **Effort C3 — painted writing-cursor mode (Medium-Large; DEFER until wanted).**
  Brightness/dim, richer shapes, painted modal-input carets, focus events, idle-settle
  blink (a `SUBSYSTEMS` row, edge-armed by input, self-clearing after N s — adopt §6's
  idle state as the design). Only worth it if brightness/theme-tinted carets prove to
  be a real user want; nothing in C1/C2 forecloses it (the theme layer already
  unifies).
- **Cut/defer:** column mode (defer; note `Selection` multi-range as the future model),
  background-pane cursor (no panes; reserve `FocusDim`, write nothing), ghost caret (no
  completion system), OSC 12 hybrid (discourage), multi-cursor UI (defer; core model
  already reserved).

### 3.4 What to do with the concept doc itself

Adopt it as a design input, not a spec: its §4 should be rewritten against Effort 9A
reality (most "settled behaviours" are shipped facts, and its open item 3 — marker
count — is answered: one pair). If accepted, move it to `docs/design/` and file the
efforts via `scripts/backlog add` (C1 naturally absorbs/supersedes B8 — same scope plus
the overlay fix).

---

## 4. Conflicts & invariant risks

### 4.1 App-owned blink vs. the idle-free law (Fork A Option B, and §4's collision blink)

The resource law (CLAUDE.md; enforced culturally by `timers.rs` + SSD-wear-class
guardrail tests) forbids level-triggered wall-clock work. A painted blinking caret is a
~500 ms perpetual wake; the §4 caret-on-marker *alternate* blink doubles down (markers
animate whenever the caret neighbors one). Reconciliations, in preference order: (a)
hardware caret — native blink, zero wakes (the C1 path); (b) painted-but-steady — no
blink at all; (c) painted + **idle-settle** — blink armed by input, self-clearing after
N s of stillness (edge-triggered, same shape as `scrollbar` fade), which is exactly the
concept's §6 idle state promoted from "team decides" to load-bearing. A blink `SUBSYSTEMS`
row without idle-settle should be rejected at review.

### 4.2 Fork B injection: the correctness checklist (the highest-stakes item)

Injection is feasible (§1.3) but every one of these must be specified and tested —
they are where data-safety and the O(visible) law live:

- **Buffer purity** — markers exist only in `marked_block`/`pending_block_begin` state
  and are injected at layout time; no path may ever write them to the buffer. (Holds by
  construction; assert it in tests anyway — save round-trip, word count, search.)
- **`visual_to_source` on a marker cell** — a click on the injected `[` must resolve to
  the marker's anchor offset (the clamp-to-neighbor pattern of the prefix region,
  `layout.rs:115`, generalized mid-row).
- **`source_to_visual` at the anchor offset** — the caret placed at the marker's offset
  must land on the *character's* cell, never the marker's (ordering rule against the
  existing "first `p.src.start >= offset`" scan; the zero-width-grapheme policy at
  `layout.rs:108–113` is the precedent for positive-width-wins tie-breaking).
- **Wrap accounting** — an injected column may push a wrap; `row_end_col`,
  desired-column vertical motion (Law 5), and the D2 caret clamp must all see the
  shifted geometry consistently. Property tests over `layout()` with/without injections
  (the F2-oracle pattern: strip injected cells → maps must be identical).
- **Highlight windows** — selection/search/diag spans are byte-ranged; injected cells
  have zero-length `src` and must not be styled as content by `overlaps()` (empty
  ranges: define whether `[` inherits the block face — probably yes, deliberately, as
  its identity).
- **Both layout paths** — per-line `layout()` AND `ventilate::layout_block` (a marker
  inside a ventilated window must appear in the stitched window ColMap with
  window-relative `src`); the S6 fill-obligation discipline (`resolve`, `origin_of`)
  already routes every consumer correctly once the maps are right.
- **O(visible)** — injection cost is O(markers-on-visible-rows) ≤ 2 (+1 pending);
  no document scan. Trivially satisfied; state it in the spec.

### 4.3 §5 background-pane cursor: designing for a nonexistent surface

No splits exist (§1.4). Adopting §5's table as written would enshrine behavior for a
feature with no roadmap item. Reduce it to a one-line reservation (unfocused contexts
use `SE::FocusDim`-family looks) and delete the rest until a splits effort exists.

### 4.4 §7's "no hardware cursor survives" vs. codebase economics

Cohesion principle 1 as written mandates painted-primary. Under the §3.1
recommendation it should be restated: *one theme model* (all cursor-family faces from
the active theme's compose ladder, mono fallbacks mandatory) rather than *one painting
mechanism*. Principles 2–6 survive intact; principle 6 (one collision rule) becomes
"hardware caret naturally overlays painted cells" on the recommended path.

### 4.5 Command-surface contract exposure

Every knob in §3 of the concept is a user-settable option and therefore MUST be
commands: shape (set-per-state + cycle), blink toggle (+ speed if C3), brightness
up/down (C3), picker-open. Persisted fields join `SettingsSnapshot` under LAW 2 with the
`every_persisted_setting_has_a_command` gate; the picker must not be the *only* route to
any state (palette exhaustiveness). The specs for C1/C3 must state contract conformance
explicitly (per the contract's own rule). C2 as B-lite touches no options (N/A
declaration); B-full likewise unless marker glyphs become configurable — recommend they
don't (role cursors are deliberately not user-configurable, which the concept settles
correctly and the contract is happy with).

### 4.6 Terminal-capability bounds (honesty items)

- DECSCUSR support varies (most modern terminals yes; some minimal ones ignore it) —
  B8/C1 must treat it as best-effort with graceful no-op, like bracketed paste in
  `term.rs`.
- Brightness interpolation (C3) is honest only at RGB depth; at 256/16/mono it degrades
  — the picker must not offer a dead control at low depth (no-silent-UI: show it
  disabled with a reason, or hide it per depth).
- OSC 12 (Option C) fails both of the above at once; recommend permanently out.

---

## 5. Open questions for the human

1. **Is brightness/dim a real want?** It is the only §3 capability exclusive to the
   painted path, and it alone justifies (or not) ever building C3. If the honest answer
   is "shape + blink is what I wanted," Fork A collapses to B8/C1 and the painted mode
   is deleted, not deferred.
2. **WordStar fidelity vs. cost on the markers (the reframed Fork B):** injected
   bracket columns that shift text (B-full, Medium, the §4.2 test matrix) or styled
   boundary cells with painted brackets only where no glyph exists (B-lite, Small,
   near-zero risk)? The data model is identical either way; this is purely how much the
   `[block]` *look* is worth.
3. **Are split panes on any horizon?** If never: strike §5's pane material entirely. If
   someday: keep the one-line `FocusDim` reservation. (Nothing in C1–C3 changes either
   way — this only decides how much of §5 survives into the adopted concept.)
4. **Off-screen marker indicator — into C2 or the cut list?** It's the one §4 open item
   with real utility (a pending begin-marker scrolled away is a genuinely lost
   landmark), but it's also the only C2 piece with new chrome-surface design (gutter vs
   edge glyph vs status segment).
5. **Does C1 absorb backlog item B8, or execute under it?** Same scope + the overlay
   caret fix; recommend absorbing (retitle B8 or file C1 and mark B8 superseded) so the
   backlog stays bijective.
