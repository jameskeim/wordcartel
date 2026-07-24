# Group ③ — degenerate-geometry safety: design spec

**Date:** 2026-07-24. **Status:** draft for Codex spec gate.
**Items:** B9 (menu-bar horizontal overflow below ~62 cols — feature) + H23 (geometry-arithmetic
u16 overflow, seed + census — debt rider).
**Branch (at execute time):** `effort-group3-geometry` off main @ `1a994ad`. No source edits
before then.
**Grounding inputs:** `scratchpad/group3/` (three explore sweeps + `fable-grounding-brief.md` +
`group3-forks.md`), every claim re-verified against the working tree at `1a994ad` (the H27
DispatchCtx merge is included; `chrome_geom.rs`, `render_overlays.rs`, and `list_window.rs` are
byte-identical across `16d0d0d..1a994ad`, and the `menu.rs`/`mouse.rs` signatures cited below are
the post-H27 forms).

All anchors are SYMBOL names (file + symbol), never line numbers, except where a specific
expression is quoted. Claims that could not be verified by reading are collected in §10.

---

## 1. Summary and locked decisions

Two backlog items, one branch, one pipeline pass. Human-locked decisions (2026-07-24, forks
resolved from `scratchpad/group3/group3-forks.md`):

1. **Scoping** — ONE effort, ONE spec. B9 is the design work (§3); H23 is a small "Part 2"
   rider (§4): a one-line fix, a recorded census, and tests.
2. **B9 = responsive label-compression ladder ("Option C")** — the bar layout is a PURE
   FUNCTION of `(area, cats)` with ZERO stored state, so the painter and all three mouse
   hit-test sites stay in lockstep by construction (the A21/H21 drift lesson made structural).
   Every category stays visible and mouse-reachable down to the narrowest rung (36 cols for
   today's 8 categories); below that, clip plus the `»` marker.
   **The abbreviation strings are the human's and LOCKED** (§3.2): File, Edit, View unchanged;
   Block→`Blk`, Format→`Fmt`, Documents→`Docs`, Settings→`Set`, Export→`Exp`.
   The ladder STRUCTURE and exact thresholds (this spec's derivation, §3.2): three discrete
   rungs — full+padded ≥ 62, full+tight ≥ 54, abbreviated+tight ≥ 36 — not per-label
   progressive shortening (rationale in §3.2.1).
3. **Overflow marker** — a single dim `»` cell (`cs.menu_closed` + `Modifier::DIM`) in the
   bar's last column whenever a category is still clipped (i.e. width below the narrowest
   rung). Visual-only: a click on the marker column does nothing, matching today's
   row-0-off-labels behavior. Painted by both the active-bar and inactive-bar arms.
4. **Dropdown shift-left-to-fit is folded in** — `menu_dropdown_rect` stops width-clamping
   against the right edge and instead shifts the box left to fit (§3.5), so right-side
   dropdowns (Export at 60 cols is a 6-column sliver today) stay readable. One shared helper;
   paint and hit-test both call it, so no drift.
5. **H23 census boundary** — fix the seed only; RECORD the audited-safe sites as this spec's
   census (§4.2–§4.3) so a reviewer sees the sweep was done and why the other sites are safe;
   add a ONE-LINE comment at the wrap-guide site naming the `wrap_column` clamp it leans on.
   Do NOT harden the invariant-distant sites (no cast-caps). The Effort-P plugin-label-length
   note (§4.3) is the durable forward artifact.
6. **H23 guard form** — plain u32 widening, NO `debug_assert`. This deviates from the filing's
   "widen … under a `debug_assert`" letter, with the human's approval: after widening, the
   intermediate is exact and the input (`w ≥ 21846`) is absurd-but-valid, not a violated
   invariant — there is nothing left to assert that is not true by construction (§4.4).
7. **Tests** — per-surface house idiom, no monolithic sweep (§6): helper-level extreme-width
   tests, a narrow-width full-render sweep extending the
   `the_prompt_detail_box_never_panics_or_overflows_a_tiny_terminal` template, paint/mouse
   agreement at a narrow width, and one moderate extreme-width full render (21846×4).
   **Threshold correction while grounding:** the boundary pair 52/53 in the resolved decision
   assumed a 7-gap separator model; the contiguous-rect model this spec adopts (§3.3) puts the
   rung-1 boundary at 53/54, so the sweep widths are `{10, 20, 35, 36, 53, 54, 61, 62, 80}`.

Command-surface contract: **N/A with reasoning** (§5).

---

## 2. Current behavior (grounded, symbol-anchored)

### 2.1 The bar layout and its two consumers

`wordcartel/src/chrome_geom.rs::menu_bar_layout_cats(area, cats) -> Vec<(usize, Rect)>` lays
category labels left-to-right from `area.x`: per label
`wgt = label.chars().count() as u16 + 2` (one space of padding each side, matching the
painter's `format!(" {label} ")`), rect `Rect::new(x, area.y, wgt, 1)`, cursor
`x = x.saturating_add(wgt)`. **No horizontal windowing and no right-edge clamp** — rects past
`area.width` are silently clipped by ratatui at paint time and unreachable by mouse.
`menu_bar_layout(area, groups)` is a thin wrapper mapping the built groups to their categories.

Consumers (all derive geometry from these two functions — the lockstep property B9 must
preserve):

- **Paint** — `render_overlays.rs::paint_menu_bar`: full-width bar background
  (`cs.menu_closed`) then one `Paragraph::new(format!(" {label} "))` per rect; the active arm
  iterates `menu_bar_layout(menu_area, &menu.groups)` (open category styled `cs.menu_open`),
  the inactive arm `menu_bar_layout_cats(menu_area, &registry::MENU_ORDER)`.
- **Mouse ×3** — `mouse.rs::mouse_menu` `Moved` arm and `Down(Left)` arm (both
  `menu_bar_layout(hit_area, groups)` + an inline
  `find(|(_, r)| ev.column >= r.x && ev.column < r.x + r.width && ev.row == r.y)`), and the
  inactive-bar `CellHit::MenuBar` arm in `handle` (same find over
  `menu_bar_layout_cats(area, &MENU_ORDER)`). Note the inactive arm passes the FULL frame
  `area` while the other two pass `chrome_geom::menu_area(area)`; the bar rects depend only on
  `x`/`y`/`width`, which `menu_area` preserves (it shrinks height only), so the rects are
  identical — the migration in §3.4 keeps whatever area each site has.

Labels come from `menu.rs::category_label` (private; `category_label_pub` is the
`pub(crate)` mirror chrome_geom calls): File, Edit, Block, Format, View, Documents, Settings,
Export — `registry::MENU_ORDER` is the fixed 8-slot array; `MenuCategory` is a closed enum
(plugins cannot add categories; `registry::menu_from_str` parses only the eight).

Widths today: labels sum to 46 chars; with `+2` padding each, the bar needs **62 columns**. At
60×24 Export paints " Expor" (verified in the B9 triage prose and by the arithmetic).

### 2.2 The dropdown right-edge sliver (the folded-in fix's target)

`chrome_geom::menu_dropdown_rect(area, groups, open)` computes the natural width
(`max leaf label chars + 2`), `list_h = leaves.min(15).min(avail_below)` where
`avail_below = area.height - 1`, returns `None` for empty leaves or `list_h == 0`, and
anchors the box at the label:

```rust
Some(Rect::new(label_rect.x, area.y + 1,
    width.min(area.width.saturating_sub(label_rect.x.saturating_sub(area.x))),
    list_h as u16))
```

The width-min clamps the box to the room RIGHT of its label. Consequences at narrow widths:
Export's label starts at x=54, so at 60 cols its dropdown is 6 columns wide; at
`area.width ≤ 54` the clamp yields a **zero-width `Some(rect)`** (paint renders a `List` into
it as a no-op; `menu_dropdown_row_at`'s `col < r.x + r.width` never matches — safe, but the
category is effectively menu-less for the mouse). `menu_dropdown_row_at` and both painters
(`paint_menu_dropdown`) share this helper, so §3.5's change lands everywhere at once.

### 2.3 The H23 seed

`chrome_geom::palette_overlay_rect(area, row_count)`:

```rust
let ov_w = (w * 3 / 5).clamp(30, 80).min(w);
```

`w * 3` in u16 overflows for `w ≥ 21846` (21846·3 = 65538 > 65535; 21845·3 = 65535 exactly, the
last safe width). Debug builds panic; release builds wrap — but the wrapped value then passes
through `.clamp(30, 80).min(w)`, so the release-mode result is *wrong but always in-range*
(cosmetic, not out-of-bounds). Every sibling overlay box inherits this line by calling the
function: `prompt_detail_rect` (width via `palette_overlay_rect(area, lines).width`),
`file_browser_overlay_rect`, and all the `*_row_at` hit-tests.

### 2.4 The existing degenerate-geometry test surface

- `render.rs::tiny_terminal_shows_notice_not_panic` — full render at (1,1)/(2,1)/(3,2).
- `render.rs::the_prompt_detail_box_never_panics_or_overflows_a_tiny_terminal` — the reusable
  template: 8 size tuples up to 200×24 through `Terminal::draw` on `TestBackend`, asserting no
  panic and the frame not resized.
- chrome_geom per-helper degenerate tests (`prompt_detail_rect_refuses_degenerate_geometry…`,
  `palette_overlay_rect_sizes_to_row_count`, `menu_dropdown_windows_a_tall_category`,
  `dropdown_indicator_row_hit_test_returns_none`, …).
- The §15.6 tiny-terminal guard in `render.rs::render` (`w < 4 || h < 2` → clamped "…" and
  return) bounds every painted frame below.

Nothing exercises the menu bar below 62 cols, and nothing exercises any width ≥ 21846 — the
seed is invisible to the suite today.

---

## 3. Design — B9: the responsive label-compression ladder

### 3.1 Shape

The bar becomes a three-rung responsive layout, selected per frame as a pure function of
`(area.width, cats)`. No stored state anywhere: no field on `MenuView` (which does not exist
for the inactive bar), nothing on `editor.mouse`. Resize self-heals because every frame
recomputes; paint and mouse cannot drift because they call the same pure functions.

```rust
/// chrome_geom.rs — the ladder rung the bar renders at, widest-first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BarRung { FullPadded, Full, Short }
```

- `FullPadded` — today's exact layout: full labels, cell text `" {label} "`
  (`wgt = chars + 2`). Chosen when the total fits.
- `Full` — full labels, single leading space: cell text `" {label}"` (`wgt = chars + 1`).
- `Short` — abbreviated labels (§3.2), single leading space: `" {short}"`. This is the FLOOR
  rung: below its total, the bar stays in `Short` and clips, and the `»` marker appears
  (§3.4).

Selection: the widest rung whose summed cell widths ≤ `area.width`. Totals are COMPUTED from
the label strings at runtime (a future 9th category shifts every threshold automatically);
the concrete numbers below are the derived values for today's 8 categories, and §6's tests
pin them so a category change consciously re-derives them.

### 3.2 § B9 label ladder — the human-review table

Abbreviations (LOCKED by the human; only long labels shorten, short ones stay full):

| Category  | Full label | Short label | Full cell (rungs 0/1) | Short cell (rung 2) | Switches to short at |
|-----------|------------|-------------|-----------------------|---------------------|----------------------|
| File      | `File`     | `File` (unchanged) | 6 / 5 cols     | 5 cols              | never                |
| Edit      | `Edit`     | `Edit` (unchanged) | 6 / 5          | 5                   | never                |
| Block     | `Block`    | `Blk`       | 7 / 6                | 4                   | **≤ 53 cols**        |
| Format    | `Format`   | `Fmt`       | 8 / 7                | 4                   | **≤ 53 cols**        |
| View      | `View`     | `View` (unchanged) | 6 / 5          | 5                   | never                |
| Documents | `Documents`| `Docs`      | 11 / 10              | 5                   | **≤ 53 cols**        |
| Settings  | `Settings` | `Set`       | 10 / 9               | 4                   | **≤ 53 cols**        |
| Export    | `Export`   | `Exp`       | 8 / 7                | 4                   | **≤ 53 cols**        |
| **Total** |            |             | **62 / 54**          | **36**              |                      |

Rung thresholds (derived: cell = pad + label chars; rung 0 pad = 2, rungs 1–2 pad = 1,
leading space only — see §3.3 for why):

| Width (cols)  | Rung        | Bar row reads                                                       |
|---------------|-------------|---------------------------------------------------------------------|
| ≥ 62          | FullPadded  | `· File ·· Edit ·· Block ·· Format ·· View ·· Documents ·· Settings ·· Export ·` (`·`&nbsp;= space; today's bar, byte-identical) |
| 54 – 61       | Full        | `· File· Edit· Block· Format· View· Documents· Settings· Export` — i.e. `" File Edit Block Format View Documents Settings Export"` |
| 36 – 53       | Short       | `" File Edit Blk Fmt View Docs Set Exp"`                            |
| 4 – 35        | Short, clipped | Short layout, tail clipped; dim `»` in the last bar column       |
| < 4           | —           | `render`'s §15.6 tiny-terminal guard returns before any bar paints  |

All five long labels switch together at the 53/54 boundary (one visual step), and every
category remains fully visible and clickable down to 36 columns.

#### 3.2.1 Why discrete rungs, not per-label progressive shortening

The alternative (shorten longest-first — Documents at ≤53, Settings at ≤48, Format at ≤43,
Export at ≤40, Block at ≤37) was considered and rejected: (a) a mixed bar
("… Blk Format View Docs Settings …") reads inconsistently — same-class items in two styles at
once; (b) it multiplies thresholds 6× for ~17 columns of mid-range benefit, and every
threshold is a test case and a human-visible mode; (c) the discrete-rung mode is a trivially
assertable property (§6 asserts the painted row text per width), whereas emergent greedy modes
are only assertable by re-running the greedy algorithm in the test — a tautology. Three rungs
keep the bar uniform at every width and the implementation two comparisons deep.

### 3.3 Layout model: contiguous rects, padding inside the cell

The current model is kept at every rung: label rects are CONTIGUOUS (`x` advances by the cell
width; no dead gap columns), and the padding is part of the rect, so the hover/open highlight
(`cs.menu_open` styles the whole rect in `paint_menu_bar`) and the mouse hit share exact cell
boundaries with zero unowned columns. Rungs 1–2 use a single LEADING space (`" {label}"`):
the bar keeps its col-0 space like today, the last label ends flush right, and — this is why
the rung-1 threshold is **54**, not the 53 a 7-gap separator model would give — 8 cells × 1
pad = 8 columns over the 46/28 label chars. (The resolved decision's 52/53 test pair assumed
the gap model; corrected in §1.7 and §6.)

New/changed surface in `chrome_geom.rs` + `menu.rs` (signatures the plan will implement):

```rust
// menu.rs — the locked short labels; exhaustive match, no catch-all (house rule).
pub(crate) fn category_label_short_pub(cat: MenuCategory) -> &'static str
// File=>"File", Edit=>"Edit", Block=>"Blk", Format=>"Fmt",
// View=>"View", Documents=>"Docs", Settings=>"Set", Export=>"Exp"

// chrome_geom.rs — the ONE producer of a bar cell's text. Layout MEASURES this string and
// paint RENDERS this string, so cell width and painted width agree by construction.
pub(crate) fn menu_bar_cell_text(cat: MenuCategory, rung: BarRung) -> String
// FullPadded => format!(" {label} "), Full => format!(" {label}"), Short => format!(" {short}")

// chrome_geom.rs — widest rung whose summed cell widths fit area.width; Short is the floor.
pub(crate) fn menu_bar_rung(area: Rect, cats: &[MenuCategory]) -> BarRung
```

`menu_bar_layout_cats(area, cats)` KEEPS its signature — it computes the rung internally and
sizes each rect from `menu_bar_cell_text(…).chars().count()` — so `menu_bar_layout` and every
existing call site compile unchanged. `paint_menu_bar`'s two arms switch from
`format!(" {label} ")` to `menu_bar_cell_text(cat, rung)` with the rung from `menu_bar_rung`
over the same cats slice each arm already uses (the active arm's `menu.groups` may in
principle hold fewer categories than `MENU_ORDER` — `grouped_commands` drops empty groups —
and the rung is simply computed per-slice; today all 8 groups are non-empty).

### 3.4 The `»` overflow marker + the consolidated bar hit-test

```rust
// chrome_geom.rs — Some(column of the bar's last cell) iff the floor-rung bar still clips,
// i.e. the last label rect's right edge exceeds the area's. None when everything fits.
pub(crate) fn menu_bar_marker_col(area: Rect, cats: &[MenuCategory]) -> Option<u16>
// = (last rect from menu_bar_layout_cats).right() > area.right()
//     then Some(area.right().saturating_sub(1)), guarded on area.width >= 1

// chrome_geom.rs — THE bar hit-test, replacing the three duplicated inline find-closures
// (mouse_menu Moved arm, mouse_menu Down arm, the inactive CellHit::MenuBar arm).
// None when (col, row) is off every label OR on the active marker column.
pub(crate) fn menu_bar_label_at(area: Rect, cats: &[MenuCategory], col: u16, row: u16)
    -> Option<usize>
```

- **Paint:** after the per-label loop in each `paint_menu_bar` arm, if
  `menu_bar_marker_col` is `Some(mc)`, render `»` at `Rect::new(mc, area.y, 1, 1)` styled
  `cs.menu_closed.add_modifier(Modifier::DIM)` (the `add_modifier` idiom used by
  `render.rs`'s fold-marker paint). The marker overwrites whatever clipped label cell was
  there — that is the point.
- **Hit-test:** `menu_bar_label_at` excludes the marker column exactly when
  `menu_bar_marker_col` is `Some` — a click on `»` falls through to "no label", which in the
  `Down(Left)` inactive-bar arm means nothing happens (today's row-0-off-labels behavior:
  "the fill area is inert") and in the active-bar arm means the existing
  click-away-closes path runs. Because paint and hit-test consult the same
  `menu_bar_marker_col`, the visual marker and the dead column can never disagree.
- **Migration:** the three mouse sites replace their inline find with
  `menu_bar_label_at(<their current area>, cats, ev.column, ev.row)` (the Moved/Down arms
  pass `menu_area(area)`, the inactive arm its full `area` — equivalent for bar geometry,
  §2.1). This is an H21-style consolidation of hand-parallel enumerations, and it is what
  makes decision 3's "click on `»` does nothing" hold at all three sites at once.

Partially-clipped labels (a label rect straddling `area.width`) remain clickable in their
visible cells and unreachable past the frame — terminals cannot report a mouse column beyond
their width, so no additional clamp is needed on the hit side.

### 3.5 Dropdown shift-left-to-fit (`menu_dropdown_rect`)

Replace the width-clamp anchor (quoted in §2.2) with a shift-left anchor:

```rust
let width = width.min(area.width);                 // never wider than the whole area
if width == 0 { return None; }                     // area.width == 0 — nothing to paint into
let x = label_rect.x.min(area.right().saturating_sub(width));
Some(Rect::new(x, area.y + 1, width, list_h as u16))
```

Bounds argument (verify while reviewing): `width ≤ area.width` ⇒
`area.right() - width ≥ area.x`, so `x ≥ area.x` and the box's right edge
`x + width ≤ area.right()` — the dropdown is ALWAYS fully inside `area` horizontally, at its
natural width whenever the terminal is at least that wide. `ratatui::Rect::right()` is
`x.saturating_add(width)` (ratatui-core 0.1.2), safe at extremes. On wide terminals
`label_rect.x + width ≤ area.right()` makes `x == label_rect.x` — today's anchor, so every
existing wide-terminal dropdown test is unaffected (checked: `menu_dropdown_windows_a_tall_category`,
`dropdown_indicator_row_hit_test_returns_none`, and the mouse.rs menu-area drift tests all use
left-anchored categories on 30–100-col areas where the min never bites).

Interaction with B9: even below 36 cols, when a clipped category is opened by KEYBOARD
(`menu::intercept` Left/Right — reach was never lost), its label rect may sit past the frame,
and the shift-left anchor still lands the dropdown fully on-screen. Invariant I3, tested in §6.

`menu_dropdown_row_at` and both painters call `menu_dropdown_rect`, so hit-test and paint move
together; no other site derives dropdown x.

### 3.6 Behavioral invariants (the whole-branch gate should probe these)

- **I1 (lockstep):** every bar/dropdown cell the painter draws and every cell the mouse
  hit-tests derive from the same pure chrome_geom functions; no consumer re-implements the
  geometry.
- **I2 (reach):** for `area.width ≥ 36` (8 static categories), every category label is fully
  visible and returns its index from `menu_bar_label_at` at some column.
- **I3 (dropdown on-screen):** any `Some` from `menu_dropdown_rect` satisfies
  `r.x ≥ area.x && r.right() ≤ area.right()`, at every width and for every category,
  including clipped-label categories.
- **I4 (statelessness):** no new state anywhere; a bare resize re-derives the entire bar.
- **I5 (marker/dead-column agreement):** `»` is painted iff `menu_bar_marker_col` is `Some`,
  and exactly that column is excluded from label hits.

---

## 4. Design — H23: the seed fix and the census

### 4.1 The fix (one line, `chrome_geom::palette_overlay_rect`)

```rust
let ov_w = ((w as u32 * 3 / 5) as u16).clamp(30, 80).min(w);
```

Exactness argument: `w ≤ 65535` ⇒ `w as u32 * 3 ≤ 196605` ⇒ `/ 5 ≤ 39321 < 65536`, so the
`as u16` narrowing is EXACT for every input — no truncation, no clamp-from-garbage. All
callers (`prompt_detail_rect`, `file_browser_overlay_rect`, every `*_row_at`, both painters)
inherit the fix through the call.

### 4.2 The census — sites audited and CLEARED (recorded so the sweep is visibly done)

The crate-wide audit (two independent grep sweeps over literal and variable multiplies, plus a
raw-add/narrowing read of every flagged site) found **exactly one u16 geometry multiply in the
shell crate** — the seed above. The triage prose's "sweep the sibling `*_overlay_rect` helpers
for the same `w*k/n` pattern" has no siblings: the siblings share the seed's line by calling
it (§2.3). The remaining classes, audited safe:

1. **All `r.x + r.width` / `r.y + r.height` comparisons** (mouse.rs ~10 sites,
   render_overlays.rs `query_area.x + query_area.width` ×4 and
   `drop_rect.y + drop_rect.height - 1`, chrome_geom `menu_dropdown_row_at`): safe by the
   **`Rect::new` saturation invariant** — ratatui-core 0.1.2 `Rect::new` computes
   `width = x.saturating_add(width) - x` (and the same for height), so every
   `Rect::new`-constructed rect satisfies `x + width ≤ u16::MAX`. All rects on these paths are
   `Rect::new`-constructed from frame areas originating at (0,0).
2. **`render.rs::place_cursor`** — all three arms already carry the H7 pattern: sums in
   usize, bounds-check `< w` BEFORE narrowing (the in-file comments cite H7); the normal-caret
   arm is additionally bounded by `text_left + text_width ≤ vp = w`
   (`nav.rs::text_geometry`) and the D2 col clamp.
3. **`render.rs::render` frame math** (`edit_top`, `status_row`, `edit_top + r`, the paint-row
   `Rect`s): bounded by the §15.6 guard (`w ≥ 4, h ≥ 2`), frame origin 0, and
   `edit_height = h - 1 - menu_rows`.
4. **`chrome_geom::file_browser_row_origin`'s `list_top + row_index as u16`**: `row_index` is
   window-relative, `< list_h ≤ 15` by the caller contract; test-only today
   (`#[allow(dead_code)]`).
5. **`splash.rs` centered render**: `lw = min(line.width, w)`, `x = (w − lw)/2`, `y < h` — all
   bounded before the adds.
6. **`mouse.rs` scrollbar math** (`erow_in_track * max_ord`): usize on 64-bit with
   `checked_div` and `.min(max_ord)` — safe.
7. **`menu_bar_layout_cats`' x-cursor**: `saturating_add`, and `Rect::new` clamps even at
   `x = u16::MAX`.

### 4.3 Safe-only-by-distant-invariant sites (recorded, deliberately NOT hardened)

1. **`render.rs` wrap-guide:** `let gx = area.x + tg.text_left + editor.view_opts.wrap_column;`
   — raw u16 adds, safe because `wrap_column ∈ [20, 9999]` is enforced in two OTHER modules
   (`config.rs` load-clamp; `prompts.rs::wrap_column_submit`) and `text_left < vp/2 ≤ 32767`,
   bounding `gx ≤ ~42766`. **This effort adds a one-line comment at this site naming the
   `wrap_column` clamp it leans on** — the only §4.3 change that ships.
2. **Narrowing label/text casts** (`menu_dropdown_rect`'s
   `max_leaf_label.chars().count() as u16 + 2`, `menu_bar_layout_cats`' label cast,
   render_overlays' palette label/chord casts): a >65533-char label would wrap the cast.
   Today's labels are command labels, chords, file/buffer names, theme names — all orders of
   magnitude below. **Forward note (the durable artifact):** Effort-P plugin-registered
   command labels are the first path that could make label length adversarial; a label-length
   cap belongs to the plugin registration boundary (Effort P), not to H23. No cast-caps here
   — hardening against strings that cannot exist yet is churn, per the resolved decision.

### 4.4 Guard-form deviation (explicit, human-approved)

The H23 filing says "widen the `*3/5` to u32 or `saturating_mul`, clamp back to u16, **under a
`debug_assert`**" (H7 parse-class stance: debug_assert + safe release clamp). This spec ships
the widening WITHOUT a `debug_assert`, deviating from the filing's letter, because after
widening there is no invariant left to assert: the §4.1 exactness argument makes the narrowing
lossless by construction, and `w ≥ 21846` is an absurd-but-valid input (PTY winsize is u16
end-to-end), not a violated invariant — `debug_assert!(ov_w <= w.max(30))` or similar would be
vacuously true and assert nothing a reviewer could ever see fire. The H7 stance's SUBSTANCE —
safe release behavior, no silent garbage wrap, no loud release panic on a render path — is
exactly what plain widening delivers. Human approved 2026-07-24.

---

## 5. Command-surface contract conformance

**N/A — this effort does not touch the command surface**, stated with reasoning because B9
touches the menu's PAINT: the compression ladder and the `»` marker change how the bar is
painted and hit-tested when narrow, not the menu's membership, commands, options, keybinding
hints, or dynamic-section rules (`docs/design/command-surface-contract.md` laws 1–8 govern
what commands/options exist and where they appear; no law addresses cell geometry). No new
command is added, and the compression is deliberately AUTOMATIC-RESPONSIVE rather than a
user-settable option — precisely so no option exists that would have to be a command,
palette-exhaustive, and menu-represented under laws 2/3/8. The dropdown shift-left changes box
placement only. The contract's invariant tests (palette-completeness,
every-option-has-a-command, hint re-resolution) are unaffected and remain green.

---

## 6. Verification design (per-surface house idiom; no monolithic sweep)

House rules honored throughout: tests assert the SCREEN or the returned geometry (the C5/S8
"render the screen, don't assert the struct" lesson), value tests live in `#[cfg(test)]`
modules beside the code, no line-number anchors. `cargo test` + workspace clippy are the merge
gates; the PTY smoke suite is run and quoted (advisory) in the pre-merge report.

**(a) Helper-level extreme width — `chrome_geom.rs` tests.**
`palette_overlay_rect` at `w ∈ {21845, 21846, 30000, u16::MAX}` (21845 = the last width whose
`w*3` fits u16 — the boundary pair documents the seed), h = 24, `row_count = 10`: no panic,
`ov_w ∈ [30, min(80, w)]`, `r.x + r.width ≤ area.right()`, `r.y + r.height ≤ area.bottom()`.
FAIL-VERIFY: run (a) against the unfixed line first — 21846 must panic in debug — then fix
(the TDD red).

**(b) Narrow-bar full-render sweep — `render.rs` tests, sibling of
`the_prompt_detail_box_never_panics_or_overflows_a_tiny_terminal`.**
Editor with `menu_bar_mode = Pinned` (bar visible, menu `None` — the inactive arm), widths
`{10, 20, 35, 36, 53, 54, 61, 62, 80}` × height 8 through `Terminal::draw` on
`TestBackend::new(w, h)`: no panic; frame not resized; and the row-0 TEXT matches the §3.2
rung for that width — at 62/80 it contains `" Documents  Settings "` (double-space padded
cells); at 54/61 it contains `"Documents Settings Export"` and NOT the double-space form; at
36/53 it contains `"Blk Fmt"` and `"Docs Set Exp"`; at 10/20/35 the LAST column cell is `»`
and the row starts `" File"`. Also asserts `»` is ABSENT at ≥ 36. (These same boundaries pin
the derived thresholds — a future label change must consciously update this table and §3.2.)

**(c) Paint/mouse agreement at a narrow width — `mouse.rs` tests.**
At 40×8 (Short rung), Pinned, menu `None`: for each of the 8 categories, a `Down(Left)` at the
label's mid-column on row 0 opens the placeholder at that `MENU_ORDER` index (the
`click_on_inactive_bar_opens_that_category` pattern, re-driven through the migrated
`menu_bar_label_at`). At 35×8 (clipped): a click on the marker column leaves
`editor.menu == None` (I5's dead column), and a click on a fully-visible label still opens it.
A hover (`Moved`) agreement case on the ACTIVE bar at 40×8 confirms the Moved-arm migration
switches categories at the compressed cells.

**(d) Moderate extreme-width full render — belt-and-braces.**
One `render()` draw at `TestBackend::new(21846, 4)` (≈87K cells): no panic, frame not resized
— proves no OTHER full-render path multiplies width after the census.

**(e) Dropdown shift-left — `chrome_geom.rs` tests.**
At `menu_area` width 60 with the real 8-category groups shape (or a synthetic groups list
whose open label starts at x ≥ 54, mirroring `tall_menu_groups`): `menu_dropdown_rect` returns
its NATURAL width (not a sliver), `r.x < label_rect.x`, `r.right() ≤ area.right()` (I3); and
`menu_dropdown_row_at` at a column inside the shifted box returns the right row (agreement).
A wide-terminal case asserts `r.x == label_rect.x` (anchor unchanged when it fits — the
no-churn claim of §3.5). FAIL-VERIFY: against the unshifted code the natural-width assertion
fails (width 6 at x=54 on 60 cols).

**(f) Regression guard on the wide bar.** The (b) sweep's 80-col case doubles as the
byte-identity check for today's bar (`FullPadded` at ≥ 62); existing menu/dropdown tests
(`menu_dropdown_windows_a_tall_category`, `dropdown_indicator_row_hit_test_returns_none`, the
mouse drift tests) run unmodified — §3.3/§3.5 predict zero churn there, and the plan treats
any needed edit to them as a spec-compliance red flag, not a test fix.

---

## 7. Acceptance criteria

1. `menu_bar_layout_cats`/`menu_bar_layout` keep their signatures; all bar geometry (rung,
   cell text, marker, hit-test) lives in `chrome_geom.rs` + the label strings in `menu.rs`;
   zero new state fields anywhere (I4).
2. At ≥ 62 cols the bar is byte-identical to today; at 36–61 all 8 categories are visible and
   clickable per §3.2's table; below 36 the dim `»` paints in the last bar column and its
   column is click-dead (I5); tests (b)/(c) green.
3. The three mouse bar-hit sites and both paint arms consume the shared helpers — the inline
   find-closures are gone (I1).
4. `menu_dropdown_rect` boxes are always fully on-screen horizontally at natural width
   whenever `area.width` allows (I3); test (e) green.
5. `palette_overlay_rect` survives `w = u16::MAX` in debug (test (a)); the wrap-guide comment
   is in place; no other arithmetic site changed (census §4.2/§4.3 is the record).
6. `cargo test` green across suites; `cargo clippy --workspace --all-targets` clean;
   `scripts/smoke/run.sh` run and its one-line summary quoted in the pre-merge report
   (advisory).
7. Backlog: B9 + H23 → `shipped` in `backlog.toml`, prose moved to `docs/backlog-archive.md`
   with the H23 archive note correcting the "sweep the siblings" framing to the §4.2 census
   result (one multiply, no siblings), then `scripts/backlog bless`.

---

## 8. Out of scope

- Per-label progressive shortening (§3.2.1 — rejected).
- A user-settable compression option (§5 — deliberately avoided).
- Hardening the §4.3 invariant-distant sites (cast-caps) — recorded instead; the Effort-P
  label-length cap note is the forward artifact.
- Clickable/scrolling semantics for `»` (visual-only by decision 3).
- The zero-width-`Some` return of `menu_dropdown_rect` at `area.width == 0` becomes an
  explicit `None` (§3.5) — but no broader degenerate-geometry redesign of the dropdown.

---

## 9. Claims not verifiable by reading (labeled; none load-bearing without a test)

1. **TestBackend cost at 21846×4** — ≈87K cells is arithmetic; the per-cell memory/time cost
   is an estimate. If test (d) proves slow in CI, shrink to 21846×2 — the assertion is
   width-driven, not height-driven.
2. **"No real terminal reports ≥ 21846 columns"** — a behavior claim about emulators; PTY
   winsize is u16, so the value is representable end-to-end and is treated as hostile input,
   per the filing.
3. **DIM rendering of `»` on user terminals** — the test asserts the style modifier in the
   `TestBackend` buffer; whether a given terminal renders DIM visibly is a
   terminal-capability question (the A21 lesson) and is covered qualitatively by the advisory
   smoke pass, not a unit assertion.
4. **Abbreviation readability** — locked by the human; §3.2's table exists for exactly that
   final look.
