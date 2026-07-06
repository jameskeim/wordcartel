# Opaque canvas + transparency toggle — design

**Status:** CLEAN — Codex spec gate r1 (READY, 3 Important folded) → r2 (empty findings), 2026-07-06. Brainstorm-approved (user approved the design and both judgment calls).
Facts cross-checked against real source at `ada0245` (branch `effort-canvas-transparency` off
`main`). Gating: per the 2026-07-06 work-style change, **Codex is the sole spec/plan gate**
(loop until clean); Fable reviews the whole branch only.

## Goal

Paint the theme's canvas (`base_bg`) across the editing area so RGB themes fully own the page —
completing the E3 chrome ladder, whose bar/overlay tones were tuned *relative to the canvas* but
currently render against the terminal's background. Make canvas painting the standard, with a
`[theme] canvas = "opaque" | "transparent"` toggle so writers who run a semi-transparent /
blurred terminal can let it show through.

## Background — why this completes E3

E3's split ladder derives bars that sink toward black and overlays that rise, *relative to
`base_bg`*. Today the editing area in **live-preview** mode (the default writing view) paints no
background — cells carry `Color::Reset` (the E3+E4 gate's I-3 finding) — so those carefully-tuned
tones sit against the *terminal's* background, which may not match `base_bg`. A `base_canvas`
helper and a canvas paint already exist for **source mode** (`compose::base_canvas`,
`render.rs:509/517/584/592`), but only there, only under glyphs, and only for RGB themes. This
effort extends a correct full-area canvas paint to both modes under one toggle.

`compose::base_canvas(theme, depth)` returns `{fg: base_fg, bg: base_bg}` at the depth. For the
terminal-* themes (`terminal-plain`, `terminal-ansi`, `no-color`) `base_bg` is `Color::Default`,
which resolves to `Reset` (terminal-default) — so those themes have **no canvas to paint**. The
implementation skips the fill whenever the canvas bg is `Reset` (avoiding a no-op paint), so the
toggle only bites on RGB-based themes, exactly mirroring how E3 gates Rgb vs non-Rgb.

## D1 — Semantics

Two modes, default **opaque**:

**Opaque (default):** the editing viewport (the document area — excluding the status/menu bars) is
filled with `base_canvas`'s background before the text lines render. Bars, overlays, and content
highlights all render normally on top. RGB themes fully own the page.

**Transparent:** the editing-viewport canvas fill is **skipped**, and the **modal-interior fill is
skipped** (`ov_fill` becomes a no-op — the `Clear` beneath each overlay already leaves
terminal-default, see-through cells). Body text renders fg-only, so the terminal shows through the
prose and the modal interiors.

**Kept in transparent mode (the usability boundary):**
- **Bar backgrounds** — status line, menu bar, scrollbar keep their chrome backgrounds. Thin chrome
  must stay readable against arbitrary terminal content. (User decision: bars stay painted.)
- **Content highlights** — selection, search match, code-block, and diagnostic backgrounds keep
  their explicit backgrounds, because these need a background to be visible/functional; a
  transparent selection is an invisible selection. (User-approved judgment call. Consequence: a
  code block shows a solid tint over the otherwise-transparent canvas.)

**Non-RGB themes** (`terminal-plain` / `terminal-ansi` / `no-color`) and **`Depth::None`** (the
monochrome/cue depth) have no canvas to paint — the toggle is a persisted-but-visually-inert
no-op for them, surfaced honestly in the toggle status (D3).

## D2 — Config axis

`[theme] canvas = "opaque" | "transparent"`, default `opaque`. An enum-valued string key living
beside `chrome` under `[theme]` — the two "how the theme's colors are applied" axes grouped
together. It is **orthogonal** to `chrome = full|zen` (that is tonal treatment; this is
opacity).

- `config::ThemeConfig` gains `canvas: Option<String>` (parsed at resolve, like `chrome`,
  `config.rs:50`); `RawTheme` gains `canvas: Option<String>` with the `if let Some(c) = rt.canvas`
  fold (`config.rs:450`).
- A `parse_canvas(&Option<String>) -> (CanvasMode, Option<String>)` helper in `theme_resolve.rs`
  mirroring `parse_chrome` (`theme_resolve.rs:62`): `"opaque"` / `"transparent"`; any other value
  (incl. `None`) → `Opaque` + an "unknown value" warning when the string was present-but-unknown.
- `CanvasMode { Opaque, Transparent }` enum in `wordcartel-core` theme (Debug/Clone/Copy/Eq),
  beside `ChromeDisposition`. (Core, because `Editor` and the render read it; parsing stays in the
  shell like `chrome`.)

**No re-derivation.** Unlike `toggle_chrome`, changing canvas does NOT re-derive or re-resolve the
theme — the chrome faces are unchanged; only the render's decision to paint the viewport and fill
modal interiors changes. So resolve does not need a `CanvasMode` parameter (contrast E3's `disp`
threading); the render reads the runtime flag directly.

## D3 — Toggle command, runtime flag, honest arms

- `Editor.canvas: CanvasMode` field (`editor.rs`, beside `chrome_disposition` at `:423`), init
  `Opaque` in `new_from_text` (`editor.rs:494`). Seeded from `parse_canvas(&cfg.theme.canvas)` at
  BOTH chrome's seed sites (Codex spec r1 Important-3): the startup seed (`app.rs:1361-1365`) and
  the baseline-snapshot construction (`app.rs:1373-1375`) — the latter so the initial mode is the
  persistence baseline. The `settings.rs` `snapshot_of` (`:138`) / `runtime_snapshot` (`:167`) also
  carry it (D5).
- `toggle_canvas` command, registered in the **Settings** category (registry) **before**
  `save_settings` (which must stay the final registration — the `journey_palette_end` invariant),
  label `"Canvas: Opaque/Transparent"`. Handler flips `editor.canvas` and sets the status; a plain
  redraw shows the effect (no rederive flag — render-only).
- **Honest status arms** (byte-exact copy, mirroring `toggle_chrome`):
  - RGB theme at a color depth: `"canvas: opaque"` / `"canvas: transparent"`.
  - Non-RGB theme, or `Depth::None`: the flip still persists, but is visually inert —
    `"canvas: transparent (no effect: {name} has no canvas)"` and the `opaque` twin
    (`{name}` = `editor.theme.name`).

## D4 — Render mechanism

One full-area canvas paint in `render()`, replacing the source-mode per-span `base_canvas`
special-casing:

1. Compute the **full edit band** — the same band the scrollbar fills (`render.rs:685`), spanning
   the whole editing area including the left/right centered-measure margins and the rows below the
   last line — NOT just the text column (`tg.text_left`/`tg.text_width`, `nav.rs:25`). The per-row
   text Paragraphs paint into `tg.text_width`; the canvas fill must cover the whole band so margins
   and blank/below-content rows are painted (Codex spec r1 Important-1).
2. If `editor.canvas == Opaque` **and** `base_canvas(theme, depth)` carries a real background
   (i.e. `base_bg` is Rgb / not `Reset` — the RGB-theme + non-`None`-depth condition), then
   `frame.buffer_mut().set_style(viewport, <canvas-bg-only style>)` **before** the line Paragraphs
   render. Fg-only text spans preserve the painted background (ratatui `Cell::set_style` patch
   semantics — the same property E3's fg-only borders rely on); explicit-bg spans (selection /
   search / code / diagnostics) override locally.
3. Drop the per-span `base_canvas` patches in the source-mode arms (`render.rs:509/517/584/592`) —
   the full-area paint now covers source mode too (and fixes its blank-line/padding gap, which the
   per-span approach never painted). Source-mode text spans become fg-only like live-preview.

**Overlay/bar interaction (transparent mode):** `ChromeStyles::build` (`render.rs:276`/`:676`)
today takes only `(theme, depth)` and derives `ov_fill` from them — it has NO canvas hook. The plan
adds a `CanvasMode` parameter to `build` (or post-processes `cs.ov_fill` at the `render.rs:676`
call site): when `Transparent`, `ov_fill` is `RStyle::default()` (a no-op), so overlay interiors
show the `Clear`'s terminal-default cells (Codex spec r1 Important-2). `ChromeStyles` is rebuilt
every frame, so this takes effect immediately with no cache to invalidate. Bars are unchanged (they always
`set_style` their own chrome bg). Content-highlight and border styling are unchanged.

**Depth:** at `Ansi16` the canvas paints the quantized named background (via `base_canvas`'s
depth-aware `to_rcolor`); at `Depth::None` `base_canvas` yields no color → nothing painted (moot,
covered by the D3 honest arm). Same Rgb/depth logic as E3.

## D5 — Persistence (per-field, mirroring `chrome`)

The E3 per-field pattern, verbatim shape:
- `SettingsSnapshot.canvas: CanvasMode` (`settings.rs`), `snapshot_of` from
  `parse_canvas(&cfg.theme.canvas)`, `runtime_snapshot` from `editor.canvas`.
- `OTheme.canvas: Option<String>` with `#[serde(skip_serializing_if = "Option::is_none")]`
  (`settings.rs:91` sibling); `MaskTheme.canvas` (`settings.rs:213`) passed through as its **own**
  per-key predicate, independent of the name-sentinel and independent of `chrome` — a canvas-only
  mask must not touch name/chrome and vice-versa.
- The bespoke `[theme]` diff arm gains a plain `diff_key` string arm for `canvas` (runtime/baseline
  stringified `"opaque"`/`"transparent"`), beside the `chrome` arm.
- The config round-trip (`config.rs:840`) extends to carry `[theme] canvas = "transparent"` and
  assert it reloads.

## Testing

- **Semantics (TestBackend cell assertions):** opaque flexoki-dark → an editing-area blank cell (and
  a below-last-line cell, and a centered-measure margin cell) carries `base_bg`; transparent → the
  same cells carry `Reset`. Bars carry their chrome bg in BOTH modes. A modal (palette) interior:
  opaque → `ChromeOverlay` bg; transparent → `Reset` (see-through). A selection and a code-block
  cell carry their explicit bg in BOTH modes (content highlights survive transparency).
- **Non-RGB inertness:** terminal-plain in both modes → editing-area cells `Reset` (no visible
  difference); the toggle persists but renders identically.
- **Unification:** source-mode rendering under opaque still paints the canvas (now via the
  full-area fill), including blank-line/padding cells the per-span approach missed. The existing
  source-mode canvas tests `source_mode_tints_canvas_for_phosphor_but_not_default` (`render.rs:2228`)
  and `source_mode_dimmed_row_keeps_phosphor_canvas` (`render.rs:2252`) are **rewritten** to assert
  the full-area-fill mechanism (the per-span `base_canvas` they were built around is removed) — they
  must still show phosphor source cells carrying the RGB canvas bg, now sourced from the band fill
  (Codex spec r1 Minor).
- **Toggle + persistence:** `toggle_canvas` flips `editor.canvas` and the honest status arms
  (RGB vs non-RGB) are byte-exact; the config round-trip carries `canvas = "transparent"`; the
  diff-law battery covers the canvas key beside name/chrome (contradiction-only removal, per-key
  mask guard); `save_settings` remains the last Settings registration (`journey_palette_end`).
- **Depth:** opaque flexoki at `Ansi16` paints the quantized named bg; at `Depth::None` paints
  nothing.

## Non-goals / deferred

- **The three-axis naming pass.** After this, three "mode" axes exist — `chrome = full|zen`
  (tonal), the E1 density presets (also sketched `zen|full`), and this `canvas = opaque|transparent`
  (opacity). Unifying their vocabulary and preset interplay is an **E1** job (E1 is the preset
  umbrella). This effort ships `canvas` as a standalone key that E1 later folds into the presets.
- **Bar transparency** (making status/menu see-through) — rejected here (Fork 1: bars stay
  painted); not revisited unless requested.
- **Per-theme opacity defaults** — the toggle is a single cross-theme user preference (like
  `chrome`), not a per-theme property.
- Anything in Theme R (editing responsiveness) or E1+E2 — separate efforts.
