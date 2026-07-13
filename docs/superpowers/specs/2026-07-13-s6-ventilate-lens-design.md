# S6 — Ventilate-as-a-lens: SPEC

**Status:** SPEC (implementation-grade), authored 2026-07-13 by promoting the human-approved S6
design in place. Every locked fork of the approved design (F1–F5, plus the seven coordinator
rulings below) is preserved and expanded to source-grounded, implementer-ready detail. **Entering
the Codex spec gate.**

**Item:** backlog **S6** — the second item of the prose-structure arc
(`docs/design/prose-structure-arc.md`: S5 → **S6** → S4 → S7 → S8). S6 is the arc's **thesis proof
or kill gate**: it renders prose one sentence per line using *wordcartel's own* detector
(`wordcartel_core::textobj::sentence_spans`, shipped in S5), so that **what the writer SEES
segmented is exactly what `select-sentence` SELECTS** — the SEE==SELECT invariant — and adds a
left rhythm gutter showing each sentence's word count. If the author uses the lens on real prose
for two weeks and turns it off, the arc stops (arc doc §4, S6 FAILURE SIGNAL).

**Grounding:** every structural and API claim in this spec was verified against the real tree by
symbol name (not line number — S5's lesson: anchors drift) during authoring, 2026-07-13. The
load-bearing facts: `RenderMode`/`View`/`line_render_for` (`wordcartel/src/editor.rs`,
`wordcartel/src/lines.rs`); the `layout()` engine and `ColMap`/`VisualRow`/`Placed`
(`wordcartel-core/src/layout.rs`); `LayoutKey` + `derive::rebuild`/`rebuild_downstream`
(`wordcartel/src/derive.rs`); the `measure` toggle end-to-end (`registry.rs`, `nav::text_geometry`);
`sentence_spans`/`sentence_bounds` (`wordcartel-core/src/textobj.rs`); `BlockKind`/`Block`/`BlockTree`
(`wordcartel-core/src/block_tree.rs`); `gather_row_ctx`/`RowCtx` and the focus-Sentence path
(`wordcartel/src/render.rs`); `count::word_count` (`wordcartel-core/src/count.rs`); and the
destructive `TransformKind::Ventilate`/`run_transform` (`wordcartel/src/transform.rs`). Line anchors
appearing below are the grounded-current ones; locate by symbol name when they drift.

**Precedent spec** (structure/rigor/house-style): `docs/superpowers/specs/2026-07-12-s5-sentence-authority-design.md`.
**Contract:** `docs/design/command-surface-contract.md`. **Arc:** `docs/design/prose-structure-arc.md`.

---

## 1. Scope — one lens, one gutter, proving one invariant

The filed item is a **non-destructive layout lens**. Toggle it on (per-buffer, default off) and the
active buffer's **paragraph** prose redraws one sentence per visual row-group, each row-group
prefixed by a fixed 6-column gutter carrying that sentence's word count. Toggle it off and the
buffer is **byte-identical** — nothing is ever written; the bytes never changed. The lens segments
with `sentence_spans`, so the sentence you see is the sentence `select-sentence` grabs, **by
construction** — this is the SEE==SELECT thesis S6 exists to prove or kill.

Three things this item is, stated against the real code so the reviewer can hold them:

| Property | Mechanism | Grounded in |
|---|---|---|
| **A layout lens, not a paint mode** | a new per-block layout path gated on `View.ventilate`, feeding a `LayoutKey.ventilate` cache field — the `measure` precedent, NOT a fifth `RenderMode` | `derive.rs` `LayoutKey`/`rebuild`; `registry.rs` `toggle_measure` |
| **Paragraph reflow across hard newlines** | gather each `Paragraph` block's full source span, join, split by `sentence_spans` | `block_tree.rs` `Block.span`; `textobj.rs` `sentence_spans` |
| **Non-destructive** | the buffer is never touched; only `line_layouts` differs; toggle-off restores byte-for-byte | contrast `transform.rs` `run_transform(Ventilate)` |

### 1.1 The destructive counterpart it must NOT be

`ventilate` already ships once — as a **destructive transform**. `TransformKind::Ventilate`
(`transform.rs:8`) drives `run_transform` (`transform.rs:326`), which builds
`repar::Options::from_par_args([--width, "--ventilate", FIXUPS_STACK]).format(input)`
(`FIXUPS_STACK`, `transform.rs:323`) and **inserts real newlines into the buffer** — an undoable
edit merged back through `dispatch_transform`/`merge_transform_into` (`transform.rs:207`+), reachable
from the `transform` "Transform…" prompt (key `'v'`, `prompt.rs:125`; registry `transform` in
`MenuCategory::Format`, `registry.rs:313`). It also cannot be reused for the lens: repar is
`&str → String` with **no offset output**, so extracting boundaries from it means round-tripping the
whole document per render — an outright `O(document)`-per-frame violation of the hot-path law
(arc doc §4, S6). S6's lens renders the **same** one-sentence-per-line SHAPE with **zero bytes
changed** and **zero repar involvement**, using core's own detector. The two coexist after S6 with
distinct ids, menus, and bindings (§8).

---

## 2. Resolved forks — decisions, with their grounding

Each fork below is a **decision**, not an open question. F1–F5 are the approved-design forks; the
seven LOCKED rulings (L1–L7) are the coordinator's 2026-07-13 dispositions of the pins this author
raised while grounding. Do not re-litigate.

### F1 — Axis: a LAYOUT lens, not a fifth `RenderMode`

`RenderMode { LivePreview, SourceHighlighted, SourcePlain, Review }` (`editor.rs:45-50`, per-buffer
`View.mode`, `editor.rs:115`) is a **paint/conceal axis only**. Its sole drawing effect is
`line_render_for(mode, is_active_line)` (`lines.rs:12-22`) → `LineRender::{RawPlain, RawStyled,
Concealed}`; it decides whether markdown markers conceal and whether inline styling paints. It
**never re-segments rows** — all four variants keep the one-logical-line → N-soft-wrap-rows mapping,
and Review renders **identically** to LivePreview (it only gates the diagnostics overlay,
`diagnostics_run.rs`). `RenderMode` is in `LayoutKey` (`LayoutKey.mode`, `derive.rs:20`) *because
conceal shifts wrap points*, not because it segments.

S6's row-segmentation is a **different axis**. Its real precedent is `measure`: a boolean **layout**
toggle that changes wrap `text_width` and therefore calls `derive::rebuild`
(`registry.rs:553-555`). S6 mirrors `measure`'s shape — a boolean flag, a new `LayoutKey` field
(added by this effort, §4.1), a rebuild on flip — and **composes with** RenderMode, measure, focus,
and diagnostics rather than
replacing any of them (§6). S6 builds **one** concrete layout lens; it does **not** build the
general layout-lens *axis* abstraction — that is E8 (`depends_on = ["S6", "S8"]`).

### F2 — Paragraph-level reflow across the author's hard newlines

The lens gathers each prose block's **full source text across its hard newlines** and splits it by
sentence. Per-logical-line re-segmentation was **rejected**: S5's R2 rule merges hard-wrapped
sentences across newlines (`textobj.rs` §4.5), so `select-sentence` already spans hard line breaks;
a per-line lens would show fragments the selection does not honor — a SEE==SELECT failure on the
git-friendly hard-wrapped prose that is the target use case. The lens exists to show actual sentence
**length** and its rhythm; a fragmented view defeats the entire diagnosis.

**Feasibility — confirmed by grounding, no new dependency.** `rebuild_downstream` already holds the
block tree at layout time (`editor.active().document.blocks()`, read at `derive.rs:263` for
`role_at`). `Block { kind: BlockKind, span: Range<usize>, children }` is fully public
(`block_tree.rs:191-199`); `BlockKind::Paragraph` is a variant (`block_tree.rs:160`); the block's
full multi-line text is `buf.slice(span)`. `sentence_spans` is pure `wordcartel-core`
(`textobj.rs:202`) and **`wordcartel-core` has no repar dependency** (Cargo.toml: pulldown-cmark,
ropey, smartstring, smallvec, unicode-*, regex-* only) — a property S6 must preserve.

### F3 — Rhythm gutter: word count only, fixed 6 columns

Per sentence row-group, a fixed **6-column** gutter: `NNN` (3-digit, **right-aligned** word count)
+ space + `│` + space + text. Right-alignment is deliberate — the digit-step 1→2→3 is a free coarse
proportional cue. **Continuation (soft-wrap) rows** of a row-group carry a **blank** numeric field
and keep only the dim `│` rule (the count belongs to the sentence, printed once). The metric is
**words** — reusing `wordcartel_core::count::word_count` (§7, L7), NOT a new tokenizer. NO
proportional bar, NO opening-word, NO opener highlighting (all judged redundant with what
one-sentence-per-line + wrap occupancy already shows; deferred, §11).

### F4 — Block scope: PARAGRAPHS ONLY (blockquotes deferred)

Ventilate + gutter apply to **`BlockKind::Paragraph` blocks only**. Every other block passes through
**verbatim** — existing per-logical-line layout, no reflow, no gutter numeric field: **blockquotes,
lists, headings, code blocks/fences, tables, thematic breaks.** Rationale: every gutter number must
mean "words in a real flowing prose sentence."

**Blockquotes are DEFERRED** (ruling L2), NOT merely fallback-gated. The technical reason, recorded
here so it is not re-litigated: a `BlockQuote` leaf `Paragraph` has a **contiguous** source span, so
a multi-line gather necessarily contains the interior `> ` prefixes of its continuation lines. That
pollutes three things — (i) the detector sees stray `>` tokens mid-sentence (the markup-blindness
residue S5 recorded, its spec §10); (ii) the painted sentence text would show literal `> `
mid-sentence, because `md_parse::analyze` conceals only a **line-leading** marker and an interior
post-newline `>` is no longer line-leading; and (iii) stripping those interior prefixes is **not
byte-length-preserving**, which breaks the ColMap-offset invariant (§5.2) that makes the paragraph
path clean. Blockquotes are a clean later extension once an offset-remapping strategy exists; they
are treated exactly like lists for S6. (Verbatim-row alignment under the lens: see §5.4.)

### F5 — State scope: PER-BUFFER, on `View`

The effort **adds** a per-buffer flag `View.ventilate: bool` (beside the existing `View.mode`,
`editor.rs:111-119`, which currently has no `ventilate` field), **NOT** a field on the
editor-global `Editor.view_opts: ViewConfig`. Rationale (human): a lens is into **this**
writing; other buffers may be reference material and must not be re-viewed. It fits the code —
`View.line_layouts` and `LayoutKey` are already per-`View`. **Default off** (do not surprise on
open; arc law D7 — nothing in this arc is on by default). Because it is per-buffer session state
(like `View.mode`), it is deliberately **NOT** a `SettingsSnapshot` field (`settings.rs:37-60` has
no per-buffer entries) and has **no config key** — so the Law-2 persisted-setting guard does not
cover it, exactly as it does not cover `View.mode` (§8).

### The seven LOCKED rulings (2026-07-13)

- **L1 — Raw-reveal = Option C.** No raw-markdown reveal on ventilated PROSE rows: their markers
  stay concealed **even at the caret**. Scoped to prose ONLY — verbatim lines in and around the view
  keep the normal active-line raw reveal (`line_render_for(mode, l == active_line)`; §3.3 step 3,
  §4.2). (§6.1 gives the full composition story; §5.3 the editing consequence; `LayoutKey.active_line`
  is inert for ventilated prose but stays effective for verbatim — §4.2.)
- **L2 — Scope = paragraphs only.** Blockquotes deferred (F4 above; recorded in §11).
- **L3 — Perf contract wording:** "per-keystroke layout work bounded by the **visible range**;
  **never** `O(document)`; **off-screen blocks never gathered**." Strict per-block memoization is an
  explicitly **optional** plan-phase optimization, not required (§4.3).
- **L4 — Giant single-paragraph block = ACCEPT with recorded residue.** No size cap, no status
  notice (§4.4, §10).
- **L5 — No `Command` enum variant.** Registry id `toggle_ventilate` + one shared setter fn in the
  new module, structurally like the `measure` registry-closure precedent (a `register_stateful`
  row that flips a flag and rebuilds) but routing the flip through a shared setter as Law 6 requires
  — do NOT add a `commands.rs` variant (§8).
- **L6 — Naming/binding:** lens `toggle_ventilate`, label "Toggle Ventilate View",
  `MenuCategory::View`, `MenuMark::OnOff`; destructive transform stays "Transform…" in Format;
  distinct ids/menus, no shared binding; default keybind `alt-v` in CUA (verified free), WordStar
  unbound (§8).
- **L7 — Gutter clamp = `999` saturation for a ≥1000-word "sentence"; word count reuses
  `count::word_count`** (§7).

---

## 3. Architecture — the block-scoped layout path

### 3.1 The 1:1 that S6 breaks

The current pipeline is strictly **1 logical line → N wrap rows**. `layout(line, role, render,
viewport_width, heading_prefix) -> (Vec<VisualRow>, ColMap)` (`layout.rs:244`) takes exactly one
logical line; `derive::rebuild_downstream` (`derive.rs:166`) walks visible logical lines from
`view.scroll`, calls `layout()` per line, and stores the result in
`view.line_layouts: BTreeMap<usize, (Vec<VisualRow>, ColMap)>` (`editor.rs:118`) **keyed by logical
line index**. F2 (paragraph reflow across hard newlines) breaks that 1:1 — one paragraph joins N
logical lines into M sentence row-groups. This break is the central structural cost of S6, and it is
contained to a **new layout path active only when `View.ventilate` is on**.

### 3.2 The new module

Block classification + row-group emission live in a **cohesive new module** (pure where possible):
propose `wordcartel/src/ventilate.rs` (name at plan discretion). It exposes one entry the derive
layer calls, and holds the gather/classify/split logic + the shared setter (L5). `derive::rebuild_
downstream` gets a **thin branch** — `if view.ventilate { ventilate::fill_block_path(editor, …) }
else { <existing per-line loop> }` — never an inline body (module-structure GATE, §9). The gutter
word count reuses `count::word_count`; the sentence split reuses `sentence_spans`; the soft-wrap
reuses core `layout()`. S6 writes **no new wrap engine and no new tokenizer.**

### 3.3 The fill algorithm (normative)

When `View.ventilate` is on, the visible-range fill (replacing the `derive.rs:257-272` loop) walks
the **fold-visible** logical lines from `first_line`, and at each step:

1. **Classify** the block at this line by the block tree's `role_at(line_start)`: `BlockRole::
   Paragraph` ⇒ **prose**; everything else ⇒ **verbatim** (F4). Classification is the ONLY use of the
   block tree here.
2. **Prose block —** take the window `(ps, pe) = nav::paragraph_range_at(blocks, buf, line_start)` —
   **the identical call `select-sentence` and focus-Sentence make** (§5.2, §6.4), so window and origin
   agree by construction. Gather `buf.slice(ps..pe)`; run `sentence_spans` over the **RAW** gathered
   text (do NOT strip newlines first — the semantic-hard-break veto must see the raw `\n` bytes so the
   lens's spans are byte-identical to `select-sentence`'s; §5.1). Each sentence span becomes a
   **row-group**: its DISPLAY string normalizes that span's interior `\n` → space (the ONLY permitted
   normalization, byte-length-preserving — §5.1), then it is soft-wrapped by the existing `layout()`
   engine at the **reduced width** (§3.4), the first row carrying the 6-col gutter word count,
   continuation rows blank-count + dim `│`. The row-group's `ColMap` is **window-relative**, offsets
   taken against `ps` (§5.2). A **zero-terminator window** (no `.?!…` anywhere) yields exactly ONE
   `sentence_spans` span ⇒ one row-group, one count (§5.5).
3. **Verbatim block —** the existing per-logical-line `layout()` path, laid out with the caret line's
   real `is_active` (§4.2 — the active raw-reveal STAYS for verbatim; L1 scopes the no-reveal to
   ventilated prose only), no gutter numeric field, but the 6-col gutter width is **reserved blank**
   so the text column does not shear (§5.4). (Verbatim blocks carrying a prefix glyph — lists,
   blockquotes — keep today's glyph geometry; see §5.4.)
4. Advance past the block (and past any fold-hidden body via `fold_view.next_visible`,
   `derive.rs:271`); accumulate visual-row heights against the same overscan budget the current loop
   uses (`derive.rs:250, 258, 268`).

Folds skip hidden blocks **before** gather: the walk already advances via `fold_view.next_visible`,
and folds hide whole section bodies (heading-anchored), so no half-hidden block is gathered
(assumption stated; §5.6).

### 3.4 The gutter as a `text_width` reservation

The 6-column gutter is realized as a **`text_width` reduction**: the effective wrap width a
ventilated prose sentence is laid out at is `vp_width − 6`. This is exactly the mechanism `measure`
already uses to force a narrower wrap (`nav::text_geometry`, `nav.rs:28-37`, feeding `vp_width` at
`derive.rs:230`), so wrapping recomputes with no new wrap logic. Because the reservation changes the
effective width, it MUST be reflected in the cache key: `LayoutKey.text_width` already subsumes wrap
geometry (`derive.rs:18` comment: "vp_width (subsumes wrap/gutter geometry)"), and adding
`ventilate: bool` to the key (§4.1) guarantees a flip re-lays-out. The **painting** of the gutter
cells (the `NNN │ ` glyphs, and the dim `│` on continuation rows) is a **seam left to the plan**:
the three candidate mechanisms are (a) an optional prefix parameter threaded into `layout()`
(mirrors its existing `prefix_glyph`/`prefix_width` machinery, `layout.rs:284-297`), (b) a
post-process pass over the emitted `VisualRow`s in the ventilate module, or (c) render paints the
gutter cells into the reserved left columns in `paint_rows`. The **requirement** the plan must honor:
the reserved 6 columns are consistent between geometry (wrap width) and paint (gutter glyphs), and
the continuation-row `│` is present. Mechanism choice is the plan's; the invariant is this spec's.

### 3.5 What is reused, verbatim

- **`layout()`** (`layout.rs:244`) — the UAX-14 soft-wrap engine, unchanged, called per sentence
  span at the reduced width.
- **`sentence_spans`** (`textobj.rs:202`) — content-only spans, allocation-free, `O(bytes)`, no
  repar dependency. The lens's authority == the selection's authority == SEE==SELECT.
- **`count::word_count`** (`count.rs:6`) — UAX-29 word segments, alphanumeric-first-char rule; the
  gutter metric (§7).
- **`BlockTree` / `Block.span`** (`block_tree.rs`) — already in hand at layout time.

---

## 4. Cache, key, and the perf contract

### 4.1 Extend `LayoutKey` with `ventilate: bool`

This effort **adds** a field `pub ventilate: bool` to `LayoutKey` (`derive.rs:11-22`, which has no
such field today), set from the newly-added `View.ventilate` (§F5, also an addition — `View`,
`editor.rs:111-119`, has no `ventilate` field today) at the single construction site
(`derive.rs:232-242`). Consequence,
free: a ventilate flip changes the key ⇒ the gate (`derive.rs:243-245`) misses ⇒ the fill re-runs ⇒
the cache is rebuilt in the new segmentation. This is the identical mechanism the `mode` and
`text_width` fields already provide, and the `layout_gate_reruns_on_each_input` test
(`derive.rs:656`) is the template a `ventilate` re-run case joins.

### 4.2 `active_line` is inert inside ventilated PROSE (but stays effective for verbatim)

`LayoutKey.active_line` exists so the caret's logical line re-lays-out raw (LivePreview reveals
markers on the active line, `lines.rs:18`). Under L1 (no raw reveal on ventilated PROSE rows) the
active line inside a prose block does **not** switch to raw — that prose block's rows stay
concealed-clean. So `active_line` is **inert for ventilated prose**: it still keys verbatim blocks
(which keep the active-line raw reveal, §3.3 step 3) and the
non-ventilate path unchanged, but it never changes a ventilated prose block's layout. This is a
deliberate no-op, not a bug; the key field stays (verbatim blocks and the off path need it), and the
fill simply does not consult per-line active state when segmenting a prose block. Note it so a
reviewer does not "fix" a supposed missing active-line handling.

### 4.3 The perf contract (L3)

**Per-keystroke layout work is bounded by the visible range; never `O(document)`; off-screen blocks
are never gathered.** The fill walks from `first_line` and stops at the overscan budget exactly as
the current loop does (`derive.rs:250, 258`), so it gathers only blocks intersecting the viewport.
`sentence_spans` is `O(bytes in the gathered text)` and allocation-free; `count::word_count` is
`O(bytes)`; both run only over on-screen prose. Editing inserts/deletes real bytes ⇒ the block tree
reparse is the existing incremental `O(edited)` path (`derive.rs:95-146`), and the fill re-runs over
the visible range only. **Strict single-block memoization** (re-lay only the edited block, reuse
other blocks' row-groups across keystrokes) is an **optional** plan-phase optimization — the visible
range is already the hot-path bound the project's law requires (CLAUDE.md: per-keystroke work
`O(visible)+O(edited)`), so S6 is not obligated to add a finer cache than `line_layouts` has today.

### 4.4 Giant-block residue (L4 — accepted, no cap)

When the lens is on and the caret/viewport sits inside a single pathological unbroken paragraph, the
gather is `O(that one block)` — because even a screenful of *rows* is sliced from the whole block's
text. This is **accepted with recorded residue**: no size cap, no status-line notice. The cost lands
**only** in the summoned lens (default off), **only** in that one block, and only while it is
on-screen (arc law D7 — the cost lands in the summoned view). A normal document of ordinary
paragraphs never hits it. Recorded in §10.

---

## 5. Cursor, selection, editing, data safety

### 5.1 The order that protects SEE==SELECT: segment RAW, then display-normalize per span

**Invariant (load-bearing): segment on the RAW block window, then normalize interior `\n` → space
ONLY inside each already-segmented span's DISPLAY string.** Do NOT strip newlines before
`sentence_spans`.

The reason is SEE==SELECT itself. `select-sentence` and focus-Sentence run the detector on **raw**
text: `gather_row_ctx` slices `buf.slice(ps..pe)` and calls `sentence_bounds(&win, head − ps)` on
raw bytes (`render.rs:503-505`), and S5's **semantic-hard-break veto** (`textobj.rs` §4.5, the
`semantic_hard_break` predicate — two-trailing-spaces or a backslash before `\n` ⇒ a GLOBAL merge
veto, nothing merges across it) inspects those `\n` bytes and the two spaces/backslash that precede
them. If the lens stripped `\n` → space **before** segmenting, the veto could no longer fire, so the
lens would **merge** spans that `select-sentence` keeps **separate** — a verse couplet or an
address-block line would collapse into one row-group while the selector still splits it ⇒ SEE≠SELECT.

So the fill segments the **raw** gathered window — the SAME window shape `select-sentence` uses
(§6.4) — producing spans **byte-identical** to `sentence_spans` on raw text. The semantic-hard-break
veto therefore governs the VIEW identically to the SELECTION, by construction. Only **after**
segmentation, when building each span's painted `display` string, are that span's interior `\n`
bytes rendered as a single space (`layout()` treats its input as one logical line and would
otherwise place a `\n` as width-0 content in `display`).

That display-time substitution is the **ONLY** permitted normalization, and it is
**byte-length-preserving** (`\n` and space are both one byte) — so every `ColMap.src` offset the
layout produces still indexes the **live buffer** correctly, with no offset remapping. A space is
whitespace, so `layout()`'s hang rule (`layout.rs:317-320`) treats it as a soft-wrap opportunity —
which is exactly right, since the author's hard newline was a wrap point. (This
byte-length-preservation is precisely why blockquotes are out — their interior `> ` strip is NOT
length-preserving; §11.)

### 5.2 Block-spanning, anchor-keyed `ColMap`

A ventilated prose block's row-groups collectively carry a `ColMap` whose `src` offsets are
**window-relative, taken against `ps` — the START returned by the SAME `nav::paragraph_range_at`
the selector and focus use.** This is the load-bearing SEE==SELECT binding, and it is a mechanism
choice, not a naming detail: `select-sentence` and focus-Sentence segment their sentences over
`paragraph_range_at`'s window — `render.rs:503-506` / `commands.rs`'s `Scope::Sentence` arm both do
`let (ps, pe) = paragraph_range_at(blocks, buf, head); let win = buf.slice(ps..pe); sentence_bounds
(&win, head − ps)` — so the window's byte origin the selector uses is exactly `ps`. **The lens MUST
gather and offset against the identical `ps`.** `paragraph_range_at` (`nav.rs:655-685`) returns the
deepest leaf block's `(span.start, span.end)` when a block contains the caret, but **falls back to a
physical, `line_start`-based blank-line-delimited range** when no block does (`nav.rs:662-685`). So
`ps` is NOT always a block-tree span start — for the gap-fallback case it is a physical line start —
and pinning the lens to `block.span.start` (the block-tree offset) would make the lens DISAGREE with
the selector on exactly the fallback and any indent-divergent cases. **Binding both the lens's gather
window AND its ColMap/resolver origin to the same `paragraph_range_at` call makes SEE==SELECT and
focus-window-identity (§6.4) hold BY CONSTRUCTION for indented, hard-wrapped, and fallback paragraphs
alike** — the lens and the selector cannot diverge because they call one function.

**Classification vs. window are distinct.** The block tree's `role_at` is used ONLY to CLASSIFY prose
vs. verbatim (`BlockRole::Paragraph` ⇒ prose; §3.3 step 1). The segmentation WINDOW and OFFSET are
`paragraph_range_at`'s `(ps, pe)` / `ps` — never `block.span.start`, never `line_start(anchor)`. The
entry is stored in `line_layouts` **keyed at the window's first logical line** (the anchor line), but
every offset inside it is measured from `ps`. Interior logical lines of the block get **no** separate
`line_layouts` entry.

**A ventilated block entry's `ColMap` and its rows' `VisualRow.src_span` are WINDOW-RELATIVE (to
`ps`), unlike the LINE-relative meaning `src_span` carries in every non-ventilated entry.** This is
the single semantic change every consumer must respect: today a cached row's `src_span` is added to
`line_start(l)` to recover a global byte offset (the line-relative convention); under ventilate the
same field is added to `ps` instead.

**The shared resolver separates LOOKUP (by logical-line index) from OFFSET (by byte, against `ps`).
Neither uses `line_start(l)` for containment — that was the round-2 bug: for a fallback/indent-
divergent window `ps` need not equal `line_start(anchor)`, so a byte-containment test against
`line_start` could fail to resolve the very entry the origin fix exists to serve.** The two axes never
mix:

- **LOOKUP is line-index-based.** Entries stay keyed at the window's FIRST logical line. Given any line
  `l`, the resolver takes the candidate via `BTreeMap::range(..=l).next_back()` (the entry whose key is
  the greatest `≤ l`), then confirms `l` falls within that entry's **logical-line range**
  `first_line..=last_line` — a **line-INDEX comparison**, never a byte comparison against `line_start`.
  A non-ventilated per-line entry is keyed exactly at `l` (its range is the singleton `l..=l`), so the
  candidate hits trivially. A ventilated entry keyed at its anchor covers `first_line..=last_line`, so
  every interior line of the window resolves to it.
- **The line range is derived from the SAME window and stored on the entry** so lookup needs no byte
  math: when the fill emits a ventilated entry it derives `first_line = buf.byte_to_line(ps)` and
  `last_line = buf.byte_to_line(pe.saturating_sub(1).max(ps))` — the line containing the window's last
  content byte (`pe` is exclusive; guard the empty/degenerate window) — and stores
  `first_line..=last_line` alongside the `(Vec<VisualRow>, ColMap)`. (`byte_to_line` is the buffer
  accessor already used in the fill, e.g. `derive.rs:215`.) This widens the `line_layouts` value or
  adds a parallel map — a plan-phase representation choice; the requirement is that the range travels
  with the entry.
- **OFFSET is byte-based on `ps`, applied ONLY after the entry is resolved.** The resolver returns the
  entry AND its **origin**: `ps` for a ventilated entry, `line_start(l)` for a non-ventilated per-line
  entry (there the two coincide, so the ordinary path is unchanged). Consumers then compute a caret
  `in_off = head − origin` or a row's global span `origin + vr.src_span`.

**Every consumer that reconstructs a global byte offset from a cached layout MUST take both the entry
(via line-index lookup) and the `origin` from the resolver — never a raw `line_start(l)` for either.
This binds NAV and RENDER alike.** The general invariant, stated so a future consumer is not silently
missed: **any code that reconstructs a global byte offset from a cached `VisualRow`/`ColMap` must
resolve the entry by line index and take the resolver origin; `line_start(l)` is used for neither
lookup nor offset in the ventilated path.**

The consumers that MUST route through it under ventilate:

- **Nav / on-demand fallbacks** — `get_or_layout` (`nav.rs:153`) and `layout_line_on_demand`
  (`nav.rs:60`), feeding `screen_pos` (`nav.rs:82`), `clamp_snap` (`nav.rs:163`), all
  `get_or_layout`-based motions (`nav.rs:190, 223, 252, 270, 301, 354, 525, 983`),
  `rows_before_caret` (`nav.rs:532`), and the click-to-caret map (`nav.rs:983-984`).
- **`layout_line_active`** (`nav.rs:142`) — the cross-line-transition helper that lays a logical
  line out directly from `line_text` + `line_start` with **no** `line_layouts`/resolver consultation,
  used at the line-transition sites `nav.rs:198, 230, 329, 382`. Under ventilate it would silently
  reintroduce per-line geometry, so it MUST become block-aware exactly like `get_or_layout`.
- **Render / paint-side overlay consumers** — these combine a cached row's `src_span` with a
  `line_start`-based origin today and MUST take the origin from the resolver under ventilate, or every
  overlay maps to the WRONG bytes (worst on indented + multi-line blocks):
  - **Focus dimming** — `paint_rows`, `render.rs:757-762`: `g_from = line_off + vr.src_span.start`,
    `g_to = line_off + vr.src_span.end` (with `line_off = line_start(buf, l)`), fed to
    `row_is_active(g_from, g_to, from, to)` against the focus region. Wrong origin ⇒ the focused
    sentence's row-group dims and the wrong rows light (also breaks the §6.4 focus-window identity).
  - **Placed rendering + diagnostics windowing** — `row_spans_placed`, `render.rs:652, 657-658`:
    `line_off = derive::line_start(buf, l)`, then `lo = line_off + vr.src_span.start` / `hi = line_off
    + vr.src_span.end` used to window `ctx.diag_all` (and the same placed path paints search/selection/
    marked-block). Wrong origin ⇒ diagnostics, search highlight, selection, and the marked block all
    paint on the wrong glyphs.
  - **Search-window bounds collection** — `gather_row_ctx`, `render.rs:530-533`: the visible search
    window is bounded by `line_start(buf, scroll)` and `line_start(buf, max_visible + 1)` derived from
    the first/last cached `line_layouts` keys. Under ventilate those keys are block anchors, so the
    bound derivation must use the resolver's block spans (the block's `span.end` for the last visible
    block), not a raw `line_start` of an anchor line, or the window clips real matches.

Each consumer — nav AND render — MUST reproduce the SAME block-scoped geometry/origin as the cache
when the line falls inside a ventilated prose block. **A non-block-aware consumer (a fallback that
re-lays per-line, OR a paint site that keeps a `line_start` origin) is a SEE==SELECT hazard**: the
caret, the dim region, or the overlay lands on different bytes than the lens shows. This is the single
most important correctness seam in S6 and gets dedicated nav-side AND paint-side tests (§12).

### 5.3 Editing under the lens (byte-identical toggle, live re-segmentation)

- **Bytes never change** ⇒ the caret is always a real byte offset into the live buffer; the
  block-spanning `ColMap` maps (row, col) ↔ byte. Up/Down move by visual (sentence-wrap) row via the
  existing `move_down_within`/`move_up_within` (`layout.rs:524/536`) **within** a row-group's map;
  cross-group/cross-block transitions go through the anchor-resolver. Home/End act by row.
- **Editing** inserts/deletes real bytes ⇒ the incremental reparse + visible-range refill re-run,
  so the edited block **re-segments live** (a period typed mid-paragraph instantly splits a
  row-group). `sentence_spans` R1–R3 are allocation-free, so the hot path stays clean (§4.3).
- **No raw reveal at the caret** (L1): typing inside a ventilated prose block never flips that block
  to raw markers — the block stays concealed-clean. The writer who wants to see/edit raw markdown
  toggles a Source RenderMode (which, composed with ventilate, shows raw markers on every sentence
  row — §6.1) or toggles the lens off. This is a deliberate, screenshot-visible editing-feel change
  inside the lens (§13).

### 5.4 Verbatim-block alignment under the lens

While the lens is on, **glyphless** verbatim blocks (headings without the heading-level glyph, code,
tables, thematic breaks) render with the **6-col gutter width reserved blank** (no numeric field, no
`│`), so their text column aligns with ventilated prose and the left edge does not shear. The gutter
is painted by **overwriting** the layout's own reserved lead-in (`push_prefix_lead_in` already emits
`" ".repeat(prefix_width)` for a glyphless row, `render.rs:627-628`) with the gutter cells —
**reserved width == painted width**, no content shift (this is the mechanism, not a prepend; a prepend
would double-count the reserved lead-in). The active raw-reveal STAYS for verbatim (§4.2 / IMPORTANT 3):
a verbatim block is laid out with the caret line's real `is_active`, so an active heading still reveals
its raw markup under the lens.

**Glyph-carrying verbatim blocks (lists, blockquotes) keep today's exact glyph geometry** — they are
laid out with NO gutter reservation, because the layout's glyph lead-in (`render.rs:610-625`) paints
the glyph at its natural width and a 6-col reservation would desync glyph paint from the ColMap's
`prefix_width` (cursor/glyph misalignment). Consequence: a ventilated list/blockquote's text starts at
its glyph indent, ~6 columns left of adjacent paragraph text — a minor left-alignment difference,
**accepted as deferred-verbatim residue** (blockquotes are deferred by L2; list ventilation is a later
item, F4). No cursor or data risk; purely the left inset of an already-verbatim block.

### 5.5 Zero-terminator prose block

A prose block with no sentence terminator anywhere (`sentence_spans` yields exactly one span for the
whole gathered text) renders as **one row-group with one word count** — the whole block, soft-wrapped
at the reduced width, one gutter number. Stated and tested (§12) so it is not treated as an error
case.

### 5.6 Folds

The fill advances via `fold_view.next_visible` (`derive.rs:271`), so a fold-hidden block is skipped
**before** it is gathered. Folds today hide whole section bodies keyed by heading, so a block is
never half-hidden; the fill relies on that (assumption stated). A ventilated prose block that is
fully hidden contributes nothing to `line_layouts`, exactly as a hidden line does today.

### 5.7 Toggle-off ⇒ byte-identical, caret preserved

Flipping `View.ventilate` off runs the shared setter → `derive::rebuild`, which rebuilds
`line_layouts` in the per-line path; the buffer bytes are untouched (the lens never wrote), and the
caret byte offset is preserved (`ensure_visible`, `nav.rs:400`, re-anchors the viewport around the
same caret). Byte-identity is guaranteed by construction (no edit path exists) and is nonetheless
**asserted** in the idempotence test (§12) as a data-safety tripwire.

---

## 6. Rendering and composition

The lens is one boolean **layout** parameter. It composes with every existing view axis; none is
exclusive with it (S6 does not build the exclusive-layout-lens rule — that is E8).

### 6.1 With RenderMode (L1 composition story)

- **ventilate + LivePreview (or Review)** — the intended view: prose markers **concealed**, one clean
  sentence per row-group, rhythm gutter. No raw reveal on a ventilated prose row at the caret (L1;
  verbatim active lines still reveal, §3.3 step 3). This is the screenshot.
- **ventilate + SourcePlain / SourceHighlighted** — Source modes render **every** line raw
  (`line_render_for` → `RawPlain`/`RawStyled` for all lines, `lines.rs:19-21`), so the sentence
  row-groups show **raw markdown markers** on each row. This is a coherent, deliberate combination
  (see your prose with its markup, one sentence per line); it is a visible behavior S6 owns, not an
  accident (§13). Because Source renders all lines raw anyway, L1's "no active-line special-casing"
  is automatically consistent here.

`RenderMode` continues to key the cache (`LayoutKey.mode`); its conceal-vs-raw choice still shifts
wrap points, now per sentence row-group instead of per logical line.

### 6.2 With measure

`measure` sets the wrap width via `text_geometry` (`nav.rs:28-37`); the ventilated sentence wraps at
the measure width **minus** the 6-col gutter reservation. Both are boolean layout params keyed
through `LayoutKey.text_width`; they coexist (a centered, ventilated, gutter-prefixed column).

### 6.3 With focus (the composition claim rests on window identity)

Focus-Sentence already consumes `textobj::sentence_bounds` in `gather_row_ctx` (`render.rs:494-512`):
it scopes `paragraph_range_at` first, then `sentence_bounds(&win, head − ps)` within that window, and
dims non-focused rows via `row_is_active` on each row's `src_span` (`render.rs:757-763`). Under the
lens, the focused sentence's row-group stays lit and the others dim — **provided the lens's gather
window equals the focus window**. This is a hard cross-feature invariant (§6.4): the lens gather
window for a caret's block MUST equal `paragraph_range_at`'s window for that caret, so the lens and
focus segment the *same* text with the *same* detector and their spans line up exactly. Dimming
remains a **paint** modifier (`ladder_style(..., row_dim, …)`, `render.rs:583`), not a layout change,
so it composes with the lens's segmentation for free once the windows match **and** the row's global
span is reconstructed with the resolver origin. Note the two distinct requirements: (a) the gather
window must equal the focus window (§6.4), and (b) the focus-dim paint site (`render.rs:757-762`) must
take its origin (`ps`) from the shared resolver, not `line_start(l)` (§5.2) — a ventilated entry's
`src_span` is window-relative (to `ps`), so a `line_start` origin would dim the wrong rows even when
the windows agree.

### 6.4 The focus-window-identity invariant (named, tested)

**Invariant:** for any caret inside a paragraph, the lens's gather window `(ps, pe)` is byte-identical
to `nav::paragraph_range_at(blocks, buf, head)` — because the lens CALLS that exact function for its
window (and its origin `ps`), the same call `select-sentence` and focus-Sentence make. Identity holds
**by construction**: the lens and the selector cannot diverge on indented, hard-wrapped, or gap-
fallback paragraphs, because there is one window function and one origin. If a future change let the
lens derive its window any other way (e.g. `block.span.start`), focus-Sentence would dim the wrong
rows and SEE==SELECT would break on the fallback/indent cases — so the fill's window MUST remain
`paragraph_range_at`'s return, never re-derived. Tested in §12 (T-focus-window, T-indent-origin).

### 6.5 With diagnostics

Diagnostics paint through `gather_row_ctx`'s `diag_all` and the placed-path builder; they are a
`Review`-gated overlay (`diagnostics_run.rs`), a paint concern, orthogonal to segmentation. They
compose unchanged (no S6 work; noted for completeness).

---

## 7. The gutter metric — words, via `count::word_count`

The gutter number is `count::word_count(sentence_source)` (`count.rs:6`) — UAX-29 word segments
whose first char is alphanumeric, the same rule `textobj::is_word` uses. Reusing it (rather than
defining a new tokenizer) makes two properties true **by construction**:

- **RenderMode-stability:** the count is computed over the sentence's **source** bytes and counts
  only alphanumeric-initial segments, so markdown punctuation (`*`, `_`, `` ` ``, `>`) is never a
  word. The number is therefore identical in LivePreview and in Source modes — it does not flicker
  when markers reveal (§6.1). (Grounded: `count::word_count` filters
  `seg.chars().next().is_some_and(char::is_alphanumeric)`, `count.rs:8`.)
- **Coherence with the shipped status-line word count** (`render_status::word_count_segment`,
  `render_status.rs:52`, already calls `count::word_count`) — the gutter and the status line agree.

**Clamp (L7):** a sentence of ≥1000 words is not real prose; the 3-digit field **saturates to
`999`** (monotone, no format-width shift). Char-count is deferred (§11).

---

## 8. Command-surface contract conformance

Per-law against `docs/design/command-surface-contract.md` and the live gates. S6 **does** touch the
command surface (a new toggle command + a View-menu entry), so conformance is stated explicitly.

- **State:** `View.ventilate: bool`, per-buffer, default off (F5).
- **Command (Law 2, "every user-settable option is a command"; Law 10 nullary):** registry id
  **`toggle_ventilate`**, label **"Toggle Ventilate View"**, `MenuCategory::View`, registered via
  `register_stateful` with `state = |e| MenuMark::OnOff(e.active().view.ventilate)` and a handler
  closure that calls the shared setter and `derive::rebuild` — **structurally like `toggle_measure`**
  (`registry.rs:553-555`: a `register_stateful` OnOff row that flips its flag and rebuilds), **with
  one deliberate improvement:** `toggle_measure` mutates its flag INLINE in the registry closure
  (there is no shared-setter precedent for measure), whereas S6 routes the flip through **one shared
  setter fn** in the new module. This is not gratuitous — the command-surface contract's Law 6 ("one
  setter per option; profiles use it too") requires a single setter any profile/plugin path can call,
  so S6's setter is a contract-conformant improvement over measure's inline closure, not an exact
  copy of it. `derive::rebuild` on flip is kept. **No `Command` enum variant** is added (L5) —
  `toggle_measure` has none either; adding one would grow `commands.rs`, a hub already carrying a
  `too_many_lines` allow (§9). The command is **nullary** (Law 10) ✓.
- **Boolean option ⇒ single OnOff primitive (shape rule 8).** `ventilate` is 2-state, so it is a
  single **toggle** carried in the menu as its own stateful representative with `MenuMark::OnOff` —
  NOT a set-per-state + cycle (that shape is for 3+-state options like `RenderMode`). This is the
  `toggle_measure`/`toggle_chrome` precedent.
- **Law 3 (palette exhaustive):** the `toggle_ventilate` row appears in the palette automatically;
  the invariant tests `palette_is_exhaustive_over_the_registry` (`palette.rs:255`) and
  `palette_is_exhaustive_over_a_plugin_loaded_registry` (`palette.rs:271`) gate it — merge GATEs.
- **Law 4 (menu ⊆ palette):** the one menu row names a registered command (`toggle_ventilate`), so
  it is in the palette by Law 3 ✓.
- **Law 6 (one setter; profiles use it too):** the single setter fn is the only mutation path for
  `View.ventilate`; the registry closure routes through it. No bypass.
- **Law 7 (hints track the active keymap):** `alt-v` hints in CUA; WordStar shows no hint (unbound,
  below). Re-resolution on preset switch is gated by `hints_reresolve_on_preset_switch`
  (`keymap.rs:1117`) and `custom_bind_surfaces_in_menu_and_palette` — merge GATEs; a new resolution
  test pins the `alt-v` chord and WordStar's deliberate unboundness (§12, T-chord).
- **Law 2 guard (`every_persisted_setting_has_a_command`, `settings.rs:1003`):** **N/A** —
  `View.ventilate` is per-buffer session state, **not** a `SettingsSnapshot` field and **not** a
  config key, exactly like `View.mode`. It has a command anyway; the guard test's scope
  (persisted `SettingsSnapshot`/config settings) is unaffected. Stated deliberately so a reviewer
  does not read a missing snapshot field as a Law-2 violation.
- **Keybinding (L6):** default **`alt-v`** in CUA — **verified free** against the full bound-alt set
  (`keymap.rs:257-370`; `alt-v` unbound, only `alt-shift-v` = `move_block_to_scratch` is taken).
  **WordStar: UNBOUND** — no sentence/lens idiom there; Law 7 means `toggle_ventilate` still appears
  in the palette without a hint, which is contract-compliant, not a gap (the S5 `close_buffer`/
  sentence-motion precedent).
- **Naming vs the destructive transform (arc doc §7 Q2 — S6's spec answers it):** the lens
  (`toggle_ventilate`, "Toggle Ventilate View", View) and the destructive transform (`transform`,
  "Transform…", Format, `registry.rs:313`) have **distinct ids, distinct menus, and no shared
  binding**. A palette search for "ventilate" surfaces both with unambiguous labels. This is
  deliberate: one is a view you toggle, one is an edit you apply.
- **No amendment to the contract is required.**

---

## 9. Module structure (anti-regrowth GATE)

- Block classification + gather + row-group emission + the shared setter = the **new cohesive
  module** (§3.2), pure where possible (the gather/split/count is pure over `&str` + `Block.span`;
  only the `line_layouts` write touches `Editor`).
- **`derive::rebuild_downstream` gets a THIN branch** (`if view.ventilate { … } else { … }`), never
  an inline body — the fill lives in the new module. The dispatcher stays a delegation.
- The toggle enters through the existing **registry seam** (`register_stateful`), not a new
  `Command` variant or a `commands::run` arm (L5) — the A14/S5 precedent (a leaf module, no enum
  variant, no hub arm).
- **Gates:** `clippy::too_many_lines` (threshold 100) on every new fn; the `module_budgets.rs`
  hub budgets — `derive.rs` is **not** a budgeted hub today (the budgeted set is `app.rs` 1000,
  `render.rs` 900, `timers.rs` 400, `plugin/host.rs` 400, `plugin/pump.rs` 350), so the thin
  `derive` branch must not push `render.rs`'s `paint_rows` over 900 if the gutter-paint seam (§3.4)
  lands there. If the gutter paint would grow `render.rs`, prefer the module-side or layout-param
  seam. Both gates are merge GATEs.

---

## 10. Known residue — accepted

- **Giant single-paragraph block** (§4.4, L4): with the lens on and the caret inside one
  pathological unbroken block, the gather is `O(that block)`. Accepted, no cap, no notice — the cost
  lands only in the summoned lens, only in that block, only while on-screen (arc law D7).
- **Inline styling across a sentence boundary:** each row-group's soft-wrap is laid out from its own
  sentence slice, and `md_parse::analyze` runs per laid-out unit; an inline construct (e.g. an
  emphasis run) that straddles two sentences is styled per-slice, the same per-unit markup-blindness
  the current per-line pipeline already has (a `**bold**` spanning a period is rare). No regression
  vs today; recorded so it is not read as an S6 bug.
- **Blockquote deferral** (§11) — not residue so much as deferred scope, recorded there.

---

## 11. Explicitly OUT of scope for S6

- **Blockquotes** (L2/F4): deferred. Reason (recorded so it is not re-litigated): interior `> `
  prefix pollution of the gathered text + a non-byte-length-preserving strip that breaks the
  ColMap-offset invariant (§5.2). Clean later extension once offset remapping exists. Treated as a
  verbatim block for S6 (reserved-blank gutter, no reflow).
- **Lists, headings, code, tables, thematic breaks:** verbatim pass-through (F4); their gutter is
  reserved-blank (§5.4). List ventilation (marker/indent-aware sub-rows) is a separate later item.
- **Proportional bar, opening-word, repeated-opener highlighting** (F3): deferred — redundant with
  one-sentence-per-line + wrap occupancy.
- **Char-count gutter** (F3/§7): words only for S6.
- **The E8 layout-lens axis abstraction** (F1): S6 is one concrete lens.
- **Sentence motions / the destructive transform:** motions shipped in S5; the transform is
  unchanged and stays a Format command.
- **Per-buffer-vs-global diagnostics** (E-theme): captured separately.
- **Fuzz/property testing of the lens:** unit + layout + e2e fixtures suffice here, as in S5.

---

## 12. Test plan — every test, by crate and file

Core stays pure (no new core tests beyond exercising `sentence_spans`/`count`, both already tested);
the lens is a shell feature, so its tests live in the shell — the new module's `#[cfg(test)]`, the
e2e journeys (`wordcartel/src/e2e.rs`), and the derive/registry/keymap gate suites. Fixture strings
assert **slice text and row-group shape**, not hand-computed offsets, wherever practical.

| # | Test | File | Pins |
|---|---|---|---|
| **T-see-select** | **SEE==SELECT, hard-wrapped sentence.** A paragraph with one sentence hard-wrapped across two logical lines renders as exactly ONE row-group under the lens; `select_sentence` with the caret in it highlights exactly that group's byte span (span from the row-group == span from `scope_range_at`'s `Scope::Sentence`). The headline invariant. | new module `#[cfg(test)]` + e2e | §1, F2, §5.2 |
| **T-idempotence** | **Toggle idempotence.** Ventilate on → off leaves the buffer **byte-identical** (assert `buffer.to_string()` unchanged) and the caret byte offset preserved. | new module `#[cfg(test)]` | §5.7 |
| **T-colmap-roundtrip** | **ColMap round-trip at a former-newline byte.** In a ventilated block whose sentence spans a normalized `\n`→space, `source_to_visual` then `visual_to_source` round-trips the byte at the former newline; the caret at that byte lands on the correct row/col and back. | new module `#[cfg(test)]` | §5.1, §5.2 |
| **T-anchor-resolver** | **Block-aware on-demand fallback, incl. `layout_line_active`.** An interior logical line of a ventilated window, resolved on-demand via `get_or_layout`, `layout_line_on_demand`, AND `layout_line_active` (the cross-line-transition helper, `nav.rs:142`), reproduces the SAME window-scoped geometry as the cached row-group; offsets computed against `ps` (`paragraph_range_at` start), not `line_start(l)`. A non-window-aware fallback disagrees — the SEE==SELECT hazard. | `nav.rs` `#[cfg(test)]` | §5.2 |
| **T-indent-origin** | **Lens spans EQUAL `select-sentence` spans for an indented multi-line paragraph.** A 1–3-space-indented, multi-line top-level paragraph under the lens: (a) the resolver, given an INTERIOR logical line, RESOLVES to the anchor entry via line-index range membership (`first_line..=last_line`) — a byte-containment test against `line_start(l)` would fail here; (b) the returned origin equals `nav::paragraph_range_at(blocks, buf, head).0` (`ps`); (c) **the lens's global sentence spans (`ps + sentence_spans(slice)`) are byte-identical to what `select-sentence` selects with the caret in each sentence** — the SEE==SELECT proof on the indent/fallback case, not merely an origin-value check. | `nav.rs` / new module `#[cfg(test)]` | §5.2, §6.4 |
| **T-diff-spans** | **Differential vs `sentence_spans` on RAW text.** The lens's row-group grouping over a block equals `sentence_spans` over the block's **RAW** (un-normalized) source — one row-group per span, in order — so the semantic-hard-break veto governs the view identically to selection. Include a two-space/backslash hard-break fixture (verse/address line) that the veto keeps as two spans: the lens must show two row-groups, matching `select-sentence`. | new module `#[cfg(test)]` | §3.3, §3.5, §5.1 |
| **T-focus-window** | **Focus-window identity (cross-feature invariant).** For a caret in a paragraph, the lens gather window == `nav::paragraph_range_at(blocks, buf, head)` (byte-identical); an e2e with `focus=true, granularity=Sentence` shows the focused sentence's whole row-group lit and the others dim (a `Modifier::DIM` row probe on the `TestBackend` buffer, the S5 `underlined_cols`/DIM-probe pattern). | new module + `wordcartel/src/e2e.rs` | §6.3, §6.4 |
| **T-paint-origin** | **Paint-side overlays land on the correct bytes under ventilate.** For BOTH an **indented** paragraph and a **multi-line (hard-wrapped)** ventilated block, assert on the `TestBackend` buffer that (a) **focus-Sentence dimming** (`render.rs:757`) lights exactly the focused sentence's row-group and dims the others; (b) a **search match** highlight (placed path, `render.rs:652`) paints on the matched glyphs; (c) a **diagnostic** window (`render.rs:652-665`) underlines the diagnosed glyphs — each on the RIGHT bytes. A `line_start`-origin regression (indent delta or window-relative `src_span`) shifts every overlay and fails these. | `render.rs` `#[cfg(test)]` + `wordcartel/src/e2e.rs` | §5.2, §6.3 |
| **T-search-window** | **Search-window bounds under ventilate.** With matches spread across a multi-line ventilated block, the `gather_row_ctx` visible-search-window bounds (`render.rs:530-533`, derived from first/last cached keys) include every on-screen match — a raw-`line_start`-of-anchor bound would clip matches inside the block. | `render.rs` `#[cfg(test)]` | §5.2 |
| **T-classify** | **Block classification + verbatim pass-through.** Paragraph → ventilated with gutter; heading/list/code/table/thematic-break/**blockquote** → verbatim, reserved-blank gutter, no reflow. | new module `#[cfg(test)]` | F4, §5.4 |
| **T-gather-newline** | **Segment-raw-then-display-normalize order.** A hard-wrapped paragraph gathers to its full span; `sentence_spans` runs on the RAW text; only each span's DISPLAY string turns interior `\n` → space; byte length unchanged and `ColMap.src` offsets index the live buffer. Assert the ORDER: stripping before segmentation would drop the hard-break veto (covered by T-diff-spans' verse fixture). | new module `#[cfg(test)]` | §5.1 |
| **T-gutter** | **Gutter: word count, right-align, continuation blanking, `999` clamp.** Count == `count::word_count(slice)`; a ≥1000-word sentence shows `999`; continuation rows blank-count + dim `│`; count identical in LivePreview and Source modes (RenderMode-stability). | new module `#[cfg(test)]` | F3, §7 |
| **T-zero-term** | **Zero-terminator block.** A paragraph with no `.?!…` renders as one row-group, one count. | new module `#[cfg(test)]` | §5.5 |
| **T-key-rerun** | **`LayoutKey.ventilate` re-run.** Flipping `view.ventilate` re-runs the layout fill (joins the `layout_gate_reruns_on_each_input` cases, `derive.rs:656`); a settled second rebuild with the flag unchanged skips the loop. | `derive.rs` `#[cfg(test)]` | §4.1 |
| **T-perf** | **Visible-range perf guardrail.** An edit inside a ventilated block in a large document relays only the visible range — no document walk (assert via `LAYOUT_RUNS` and a fill-scope probe: off-screen blocks are not gathered; the fill count is bounded by the visible range, not total blocks). | `derive.rs` / new module `#[cfg(test)]` | §4.3 |
| **T-compose-source** | **Composition e2e.** ventilate + LivePreview = concealed clean rows; ventilate + SourcePlain = raw markers on each sentence row; ventilate + measure = reduced wrap width. | `wordcartel/src/e2e.rs` | §6.1, §6.2 |
| **T-registry** | **Command + menu.** `toggle_ventilate` registered (`MenuCategory::View`, `MenuMark::OnOff`), flips `View.ventilate`, calls `derive::rebuild`; dispatch through the registry works; palette-completeness gates (`palette.rs:255/271`) pass with the new row. | `registry.rs` `#[cfg(test)]` | §8 |
| **T-chord** | **Chord resolution.** CUA resolves `alt-v` → `toggle_ventilate`; WordStar resolves it to no command (deliberately unbound, the S5 pattern); hint re-resolution gates (`keymap.rs:1117`) stay green. | `keymap.rs` `#[cfg(test)]` | §8 |

**Contract invariant GATEs in force (unchanged, must pass):** `palette_is_exhaustive_over_the_
registry` + `_plugin_loaded_registry` (`palette.rs:255/271`), `every_persisted_setting_has_a_command`
(`settings.rs:1003`), `hints_reresolve_on_preset_switch` (`keymap.rs:1117`) +
`custom_bind_surfaces_in_menu_and_palette`. Plus the standing gates: `cargo test` all suites,
warning-free build for touched crates, workspace clippy clean (`too_many_lines` at 100), module
budgets, and the PTY smoke suite (mandatory-run / advisory-pass, one-line summary quoted in the
pre-merge report).

---

## 13. Behavior change to SURFACE, not hide

Visible changes S6 ships, to be called out in the effort notes and release notes:

- **A new per-buffer lens** (`alt-v` / View ▸ Toggle Ventilate View), **default off**. On: paragraph
  prose redraws one sentence per row-group with a word-count gutter.
- **On a ventilated PROSE row, markers stay concealed even at the caret** (L1) — a deliberate
  editing-feel change from the normal LivePreview active-line raw reveal. This is scoped to
  ventilated-prose rows ONLY: verbatim lines in and around the view keep the normal active-line raw
  reveal (`line_render_for(mode, l == active_line)`, §4.2 / §3.3 step 3) — an active heading still
  shows its `# `. To edit raw markdown of a prose sentence under the lens, use a Source RenderMode
  (which then shows raw markers on every sentence row) or toggle the lens off.
- **ventilate + Source modes** show raw markers on each sentence row-group (§6.1) — coherent, owned.
- Nothing else changes when the lens is off: the per-line pipeline, all motions, focus, measure, and
  diagnostics behave exactly as before (the flag defaults off and gates the entire new path).

---

## Pipeline status

**Brainstorm: COMPLETE and approved (2026-07-13), all forks F1–F5 + rulings L1–L7 locked by the
human.**
**Spec: AUTHORED (this document, 2026-07-13) — entering the Codex spec gate.**

**Not yet done:** Codex spec gate (re-run until clean) → plan (`superpowers:writing-plans`) → Codex
plan gate → branch `effort-s6-ventilate-lens` → subagent-driven TDD execution → the two final gates
(Fable whole-branch + Codex pre-merge) → `--no-ff` merge.

---

## History

- **2026-07-13 — spec authored** by promoting the approved S6 design in place. Grounded against the
  real tree by symbol name (RenderMode/View/line_render_for; layout()/ColMap/VisualRow; LayoutKey +
  derive::rebuild; the measure toggle + nav::text_geometry; sentence_spans/sentence_bounds;
  BlockKind/Block/BlockTree; gather_row_ctx + focus-Sentence; count::word_count; TransformKind::
  Ventilate/run_transform).
- **2026-07-13 — seven rulings LOCKED by the human** (folded in as L1–L7): (L1) raw-reveal Option C —
  no raw reveal inside ventilated blocks, `active_line` inert there; (L2) scope paragraphs-only,
  blockquotes deferred with the interior-`> `/non-length-preserving-strip reason recorded; (L3) perf
  contract = "visible-range bound, never O(document), off-screen blocks never gathered," strict
  per-block memoization optional; (L4) giant-block accepted with recorded residue, no cap/notice;
  (L5) no `Command` enum variant — registry `toggle_ventilate` + one shared setter (structurally
  like the measure registry closure, but routing through a shared setter per Law 6); (L6)
  naming/binding — "Toggle Ventilate View" in View, `alt-v` (CUA, verified
  free), WordStar unbound, distinct from the Format "Transform…" command; (L7) gutter clamp `999`
  saturation, word count via `count::word_count`.
- **2026-07-13 — spec invariants bound** from the author's scoping memo: segment on RAW text then
  display-normalize interior `\n`→space per span (the only permitted normalization, byte-length-
  preserving); ventilated entry keyed at its first logical line but offsets taken against `ps` (the
  `paragraph_range_at` window origin — the selector's origin), with a shared window-aware resolver
  that supplies the ORIGIN to every nav AND render consumer reconstructing a global byte offset from a
  cached row (never raw `line_start`); the
  gather window equals `paragraph_range_at`'s window (focus composition); zero-terminator block ⇒
  one row-group; folds skip hidden blocks before gather; the gutter-continuation-`│` paint mechanism
  is named as a plan-phase seam (layout() prefix param vs post-process vs render-paints-cells), the
  reserved-column consistency invariant is this spec's.
- **2026-07-13 — Codex spec gate round 1 (NOT-READY) folded in** (five findings; no approved
  decision changed — all protect the approved invariants): (C1) **segment RAW, then display-normalize
  per span** — stripping `\n` before `sentence_spans` would destroy S5's semantic-hard-break veto and
  merge spans `select-sentence` keeps separate ⇒ SEE≠SELECT; §5.1 rewritten, §3.3 step 2 reordered,
  T-diff-spans/T-gather-newline updated. (C2) **offset origin = `block.span.start`, not
  `line_start(anchor)`** — pulldown-cmark starts a paragraph node after 1–3 leading spaces, so the two
  differ for an indented paragraph; §5.2 corrected, "verified" claim removed, T-indent-origin added.
  (I1) **`layout_line_active` (`nav.rs:142`, sites 198/230/329/382) added to the resolver-consumer
  set** — it lays out a line directly with no `line_layouts` consultation and would reintroduce
  per-line geometry under ventilate; §5.2 + T-anchor-resolver updated. (I2) **`View.ventilate` and
  `LayoutKey.ventilate` reworded as ADDITIONS**, not verified-present fields (§F1, §F5, §4.1). (Minor)
  **the shared setter reworded** as a Law-6-required improvement over `toggle_measure`'s inline
  closure, not an exact mirror (§8, L5).
- **2026-07-13 — Codex spec gate round 2 (NOT-READY) folded in** (one new Important; round-1 fixes all
  held; no approved decision changed). **The block-aware-consumer invariant was completed on the
  RENDER side.** §5.2 previously enumerated only nav/on-demand consumers, missing the paint-side sites
  that also combine a cached row's `src_span` with a `line_start` origin — under S6's block-anchored,
  block-relative `src_span` these would map overlays to the wrong bytes (worst on indented +
  multi-line blocks). Fix: (a) §5.2 now states the **resolver supplies the ORIGIN**
  (`block.span.start` for ventilated entries, `line_start` otherwise) and that a ventilated entry's
  `ColMap`/`VisualRow.src_span` are **block-relative** (vs line-relative in non-ventilated entries);
  (b) a **general invariant** added — "any code that reconstructs a global byte offset from a cached
  `VisualRow`/`ColMap` must use the resolver origin" — so a future consumer is not silently missed;
  (c) the three render consumers enumerated explicitly — focus dimming (`render.rs:757`), placed
  rendering + diagnostics windowing (`render.rs:652`), search-window bounds (`render.rs:530`); (d)
  §6.3 notes the two distinct focus requirements (window identity AND resolver origin at the dim
  paint site); (e) tests **T-paint-origin** (focus dim / search highlight / diagnostic window on the
  right bytes, for BOTH an indented and a multi-line block) and **T-search-window** added.
- **2026-07-13 — Codex spec gate round 3 (NOT-READY) folded in** (one Important; an internal
  inconsistency the C2 fix left behind; no approved decision changed). The round-2 §5.2 resolver
  defined LOOKUP as "the entry whose `block.span` **contains** `line_start(l)`" — but for the indented
  paragraph C2 exists to handle, `block.span.start` sits AFTER the leading spaces, so `line_start
  (anchor)` is OUTSIDE `block.span` and the byte-containment lookup would **fail to resolve** the
  anchor entry, so the corrected `block.span.start` origin was never reached. Lookup-key and
  offset-origin had been fixed inconsistently. Fix: §5.2 now **separates LOOKUP (line-index) from
  OFFSET (byte)** — entries stay keyed at the block's first logical line; the resolver maps any line
  via `range(..=l).next_back()` then confirms membership in the entry's stored **logical-line range**
  `first_line..=last_line` (a line-INDEX comparison, never a byte comparison against `line_start`); the
  range is computed at rebuild time (`byte_to_line(block.span.start)` .. `byte_to_line` of the last
  content byte) and travels with the entry; `block.span.start` is the origin used ONLY after the entry
  resolves; `line_start(l)` is used for **neither** lookup nor offset in the ventilated path.
  T-indent-origin strengthened to assert the anchor entry actually RESOLVES for an interior line of an
  indented paragraph (not only that the origin value is right).
- **2026-07-13 — Codex PLAN gate round 1 (NO-GO) folded into the spec** (one Critical origin-mechanism
  fix; no approved decision F1–F5/L1–L7 or user-visible behavior changed — it PROTECTS SEE==SELECT).
  **Origin rebound from `block.span.start` to `ps` — the START returned by the SAME
  `nav::paragraph_range_at` the selector and focus use** (verified `render.rs:503-506`). Reason:
  `paragraph_range_at` (`nav.rs:655-685`) falls back to a physical `line_start`-based range when no
  block contains the caret, so `block.span.start` (the block-tree offset) would make the lens DISAGREE
  with the selector on fallback/indent-divergent paragraphs. Now the lens gathers over, and offsets
  against, the identical `paragraph_range_at` call — so SEE==SELECT and focus-window-identity hold BY
  CONSTRUCTION for indented, hard-wrapped, and fallback cases. Block-tree `role_at` is retained ONLY
  for prose-vs-verbatim CLASSIFICATION. Changed: §3.3 (window = `paragraph_range_at`, not
  `buf.slice(block.span)`), §5.2 (origin = `ps`, window-relative `src_span`, resolver range from
  `byte_to_line(ps)`/`byte_to_line(pe−1)`), §6.3/§6.4 (identity by construction), §5.4 (gutter
  OVERWRITES the reserved lead-in — reserved width == painted width, no double-count; verbatim active
  raw-reveal stays; glyph-carrying verbatim keeps today's geometry, a minor left-inset accepted as
  deferred residue), §3.3 step 3 (verbatim laid out with the real `is_active`, L1 scoped to prose),
  and the T-anchor-resolver / T-indent-origin / T-paint-origin rows (T-indent-origin now asserts lens
  spans EQUAL `select-sentence` spans on the indented case). The plan's IMPORTANT 1–4 + MINOR are
  folded into the plan doc's own round-1 note.
- **2026-07-13 — Codex plan gate round 2 (sweep residue).** Swept three stale current-invariant
  phrasings the round-1 origin fold missed: the "spec invariants bound" entry now says offsets are
  against `ps` (not `block.span.start`); §13's behavior-change bullet and §4.2 are scoped so "markers
  stay concealed even at the caret" applies to ventilated PROSE rows only (verbatim keeps active-line
  raw reveal). No logic/decision change.
