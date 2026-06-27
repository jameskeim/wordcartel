# Wordcartel — Markdown Theming — Design

**Date:** 2026-06-26
**Status:** Design (pre-plan) — Codex spec-reviewed twice (17 + 10 findings folded in)
**Effort:** Theming (standalone; after Effort 5). **Closes** the v1 §13.2
accessibility item (color-independent legibility) + the §5-backlog "configurable
themes." Scope **B** (no-color path fully cued: heading-level glyph + document-
selection painting). Built as **one design, three independently-green plans** (§12).
**Parent spec:** `docs/superpowers/specs/2026-06-21-wordcartel-design.md` (§13.2; §5 Backlog; §3.11 render modes; §13 construct set).
**Coverage ledger:** `docs/superpowers/plans/2026-06-22-wordcartel-coverage-ledger.md`

## 1. Summary

Markdown in wordcartel is nearly colorless: only inline `Style`
(emphasis/strong/code/link) gets color, **block roles get none**, and render.rs
hardcodes ~21 scattered `Color::` literals with no central palette.

This effort adds **themes**: a theme = **palette + a markdown-element→style
mapping**, pure data in `wordcartel-core`, resolved to ratatui in the shell
through one composition seam. Built-ins: **Default** (reproduces today's color
sites), **No-color/high-contrast**, **Tokyo Night**, and a **phosphor monitor
family** (green/amber/red/blue/purple, each in a **shaded** and a **flat**
variant — 10 themes). Users can also select **any base16/base24 palette**, override
individual elements in config, and the renderer **auto-downsamples** to the
terminal's color depth.

Two **core producer additions** widen what's themable: **YAML front matter** (a
byte-0 `FrontMatter` role) and **comments** (`<!-- -->`, block + inline → a
`Comment` element). **Structural glyphs** give elements a non-color cue —
blockquote `▎`, thematic-break `───`, and a theme-controlled heading-level shade
glyph (`█▓▒░▏·`) — all routed through a new **cursor-safe prefix geometry** so they
don't desync the caret (the 5g fold-marker trap). And **document selection** is
painted on the text for the first time.

Scope = **markdown text + on-text overlays** (search, diagnostics, focus,
**selection**) + a small **chrome palette**. Phosphor themes also **tint the
source views** (their base hue applies in source modes), giving the authentic
green/amber-on-black monitor look.

## 2. Goals / Non-Goals

### Goals
- Pure `wordcartel-core::theme` (UI-agnostic; `#![forbid(unsafe_code)]`, IO-free).
- One shell composition seam: a ratatui style from an ordered face stack.
- Built-ins: Default, No-color, Tokyo Night, **phosphor {green,amber,red,blue,purple} × {shaded,flat}**.
- **base16/base24 palette import** (one canonical mapping → hundreds of schemes), parsed **without a YAML dependency**.
- Per-element config overrides; auto color-depth degradation; live theme switch.
- **New producers:** front matter + comments (block & inline).
- **Cursor-safe prefix geometry** for all synthetic prefixes (list/blockquote/hr/heading).
- **Full §13.2:** heading-level glyph; **document-selection painting**; diagnostics distinct from links without color.

### Non-Goals (v1)
- YAML-syntax highlighting *inside* front matter (keys/values) — block face only.
- Helix/`.tmTheme`/VSCode importers; full chrome re-skin beyond the small palette;
  per-buffer themes; theme hot-reload; theme-editor UI; inline-image tinting;
  SourceHighlighted true syntax highlighting (it currently equals SourcePlain).

## 3. Architecture (functional-core / imperative-shell)

```
wordcartel-core (IO/thread-free, #![forbid(unsafe_code)])
  theme.rs    NEW — Color, Face, Theme, SemanticElement, built-ins (incl. phosphor
                    family + shade ramp), quantize, BasePalette, from_base16. Pure.
  style.rs    ~  + Style::Comment (inline).
  block_tree.rs ~ + BlockKind::FrontMatter (byte-0 only) + BlockKind::HtmlComment;
                    kind_to_role → FrontMatter / Comment.
  md_parse.rs ~  + blockquote/thematic-break prefix glyphs; inline `<!--` → Style::Comment.
  layout.rs   ~  + per-row PREFIX width in the visual geometry (§3.7) — the cursor-safe seam.

wordcartel (shell)
  theme_load.rs NEW — ResolvedTheme{theme,depth,warnings}; detect_depth; resolve_theme;
                      base16/24 parse (hand-rolled, no YAML dep); ~/relative paths.
  compose.rs    NEW — face-composition pipeline + face_to_ratatui(depth) seam.
  theme_picker.rs NEW — overlay listing themes (mirror outline_overlay).
  config.rs     + RawThemeConfig{name,file,depth,heading_level_glyph,styles}.
  render.rs     ~ replace style_to_ratatui + hardcoded Color:: with compose(stack);
                    paint Selection; fill the heading prefix glyph (content; geometry in layout).
  nav.rs/mouse.rs ~ cursor + hit-test account for the layout prefix width (§3.7).
  registry.rs   + `theme` command → picker.
  editor.rs/app.rs + active Theme + Depth seeded at startup; picker swaps Theme + relayouts.
```

### 3.1 Core: `Color`, `Face`, `Theme`

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color { Rgb{r:u8,g:u8,b:u8}, Ansi16(u8), Indexed(u8), Default } // Default == ratatui Reset

/// One look. Option None = "inherit accumulated" (§3.4); Some(Default) = hard reset.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Face {
    pub fg: Option<Color>, pub bg: Option<Color>,
    pub underline_color: Option<Color>,                 // diagnostics color the underline, not the text
    pub bold: Option<bool>, pub italic: Option<bool>, pub underline: Option<bool>,
    pub strike: Option<bool>, pub reverse: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SemanticElement {
    Text,
    Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link, // inline (from core Style)
    Heading(u8), BlockQuote, CodeBlock, ListMarker, ThematicBreak,
    FrontMatter, Comment, Selection,
    SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar, FocusDim, FoldMarker, WrapGuide,
    Chrome, ChromeSelected, ChromeMuted,                // §3.8 chrome site→face table
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Theme {
    pub name: String,
    pub base_fg: Color, pub base_bg: Color,  // the canvas; applies in source modes too (§3.5)
    pub heading_level_glyph: bool,           // show the level shade glyph in live-preview (§3.7/§4)
    faces: ThemeFaces,                       // total: a Face per element
}
impl Theme {
    pub fn face(&self, el: SemanticElement) -> Face;   // total; Heading clamps 1..=6
    pub fn builtin(name: &str) -> Option<Theme>;
    pub fn builtin_names() -> &'static [&'static str]; // default,no-color,tokyo-night,phosphor-*[-flat]
}
pub fn default() -> Theme;     // reproduces today's color SITES (base_fg/bg = Default → source unchanged)
pub fn no_color() -> Theme;    // all fg/bg/underline_color = Default; cues via modifiers/glyphs (§4)
pub fn tokyo_night() -> Theme; // MIT palette from tokyonight.nvim

/// Phosphor monitor family. `hue` = the phosphor color; `flat` = single-shade
/// (modifiers/glyphs only) vs shaded (lightness varies by element). base_bg = a
/// near-black tint of the hue; base_fg = the hue (so SOURCE views are tinted, §3.5).
pub fn phosphor(name: &str, hue: Rgb, flat: bool) -> Theme;
/// Lightness ramp of a single hue (HSL L scaling), for shaded phosphor + the no-color glyph density.
fn shade(hue: Rgb, level: u8) -> Color;
```

Hues (approx; exact in the plan): green `#33ff33`, amber `#ffb000`, red `#ff5555`,
blue `#5599ff`, purple `#cc99ff`. `face` is **total**; core has **no ratatui dep**.

### 3.2 Core: depth + quantize + base palette
```rust
pub enum Depth { Truecolor, Indexed256, Ansi16, None }
pub fn quantize(c: Color, depth: Depth) -> Color;      // Rgb→Indexed(6x6x6+grays)@256; →Ansi16@16; Ansi16/Default passthrough
pub struct BasePalette { pub base: [Color;16], pub extra: Option<[Color;8]> } // base16 or base24
pub fn from_base16(name: &str, p: BasePalette) -> Theme; // canonical markdown mapping; total even from 16 slots
```

### 3.3 Shell: resolution, depth, the seam
- `detect_depth()` (case-insensitive): `NO_COLOR`→None; `TERM` empty/`dumb`→None;
  `COLORTERM`∈{truecolor,24bit}→Truecolor; `TERM` `*-direct*`→Truecolor;
  `*256color*`→Indexed256; else Ansi16.
- **Precedence (locked): `NO_COLOR` > explicit `[theme] depth` > detection.** Effective
  depth stored separately; when `None`, the picker can't re-enable color.
- `resolve_theme(cfg) -> ResolvedTheme{theme,depth,warnings}`: effective depth →
  if None, theme=`no_color()`; else `builtin(name)` or `from_base16(parse(file))`
  (error→`default()`+warning); apply `[theme.styles]` per-field (bad hex/key→skip+warning).
  Warnings append to the existing `(Config, Vec<String>)` startup stream.
- **base16/24 parsing (NO YAML dep — Codex: `serde_yml` is deprecated/RUSTSEC-2025-0068):**
  base16 files are a flat map; a small hand-rolled shell parser reads
  `scheme`/`author` strings + `base00..base0F` (and optional `base10..base17`) as
  `key: "rrggbb"` lines (tolerant of quotes/`#`). No external YAML crate. Core stays IO-free.
- `compose.rs`: `face_to_ratatui(face, depth)` (quantize; `Default`→`Reset`;
  map 5 modifiers + underline_color) and `compose(theme, depth, stack)` (fold faces, §3.4).

### 3.4 Composition pipeline
Each `Some` field overrides the accumulator; `None` inherits; `Some(Default)` hard-resets:
```
Text(base) → block role → inline style → FocusDim(inactive) → Selection → Search → Diagnostic
```
Diagnostics stack underline + underline_color without changing fg. `ListMarker`
styles the row prefix; `FoldMarker`/`WrapGuide` their own glyphs. **Selection** is
projected onto cells via the same placed-path glyph intersection render uses for
search; any non-empty primary selection forces the placed path (Codex C1). The
style-stack builder is factored so search/selection/diagnostics share it.
Cross-products are tested in §8 (heading+code, link+diag, selection×code, selection×search, …).

### 3.5 Render-mode behavior (§3.11)
- **LivePreview:** full pipeline (roles + inline + overlays + structural glyphs + heading glyph).
- **Source modes (Highlighted/Plain, both `source_mode` today):** apply the theme's
  **`base_fg`/`base_bg` only** (the canvas) + overlays (Selection/search/diag/focus) —
  **no per-element semantic faces**, no heading glyph (the literal `#` shows). This
  honors §3.11 "zero *styling*" (no semantic differentiation) while letting the
  **phosphor base hue tint the source** (green/amber-on-black) and the **Default**
  theme leave source untouched (its `base` = `Default` = terminal). `Selection`
  paints in every mode (a cursor concern).

### 3.6 Active theme location
`Editor` gains `theme: Theme` + `depth: Depth`, seeded at startup. The picker swaps
them and **triggers a relayout** (the heading-glyph flag is a layout input, §3.7).
Resolution/relayout happen once at startup and on switch — never per frame.

### 3.7 Cursor-safe prefix geometry (the keystone — Codex D1/E1)
Today `prefix_glyph` is stored *after* layout and prepended by render as an
independent span; `ColMap` columns ignore it, so cursor/mouse use unshifted
columns — adding a glyph shifts text right without moving the caret (the 5g
fold-marker bug; Codex finds list bullets are **already latently desynced**).

Fix: **the row prefix becomes part of the visual geometry in `layout`.** `layout`
returns a per-row `prefix: { text: String, width: u16 }`; every visual column in
`ColMap` is offset by `width`; soft-wrap capacity is reduced by `width`;
`nav::source_to_visual`/`visual_to_source`, cursor placement, and
`mouse::offset_at_cell` all add `width`. All synthetic prefixes flow through this
one path:
- **list bullet `•`, blockquote `▎`, thematic-break `───`** — content-driven,
  always produced in `md_parse`/layout (theme-independent).
- **heading-level glyph** — theme-driven; `layout` takes a `heading_prefix: bool`
  input (from `theme.heading_level_glyph`), reserving a **fixed width** when on so
  geometry stays theme-independent for a given setting; the glyph *content* (which
  shade char) is filled by render. A theme switch relayouts (§3.6).

This is plan ②'s keystone; it likely **fixes the pre-existing list-bullet desync**.
Round-trip tests: cursor/mouse on prefixed, wrapped, narrow-width rows.

### 3.8 Chrome site → face table (Codex F3)
Three chrome faces cover all chrome sites; the Default theme reproduces today's look:

| Site (render.rs) | Face | Default value (today) |
|---|---|---|
| status bar; menu bar; overlay frames (palette/outline/diag) | `Chrome` | white on black |
| selected/active row (menu item, palette/outline/diag selection); status REVERSED | `ChromeSelected` | black on white (reverse) |
| muted chrome text (query prompt, inactive menu, secondary) | `ChromeMuted` | white on dark-gray |

(If the plan finds a 4th distinct combo, it adds a face; the table is the contract.)

### 3.9 Producers (Codex A/B — sound against the real parser)
- **Front matter (byte-0 only):** do **not** enable pulldown's global metadata
  option (it would let the incremental reparser misclassify a mid-document `--- … ---`
  region, breaking full/incremental oracle equivalence). Instead a dedicated
  **byte-0 prefix scan** recognizes a leading `---\n … \n---` block → a new
  `BlockKind::FrontMatter`; the remainder parses with ordinary options. Edits
  intersecting the front-matter delimiters **force a reparse from byte 0**. Oracle
  gains mid-document `--- --- ` and delimiter insert/remove cases. Only
  `BlockKind::FrontMatter` maps to `BlockRole::FrontMatter` (not the existing `Other`).
- **Comments:** add `BlockKind::HtmlComment` (a block whose source begins `<!--`) →
  `BlockRole::Comment`; generic `HtmlBlock` stays separate (we do **not** color
  `<div>` as a comment). Inline: match `Event::InlineHtml`, emit
  `StyleSpan{Style::Comment}` **only** when the source span is `<!-- … -->` (other
  inline HTML stays Plain). Adding `Style::Comment` updates the **exhaustive**
  `style_to_ratatui`/element map (+ a total-over-`Style` mapping test so a missing
  arm fails to compile/test).

## 4. §13.2 — color-independent cues
**Requirement, precisely:** when **color is absent** (effective `Depth::None`, the
No-color theme, or a phosphor theme reduced to one shade), **every distinction
carries a non-color cue**. When color is present, color may be the cue. So the
No-color theme is the proof object; colored themes layer color *over* a
cue-bearing base.

**Locked:** when effective `Depth == None`, `heading_level_glyph` is **forced on**
regardless of theme/config (Codex Part3-1) — headings stay distinguishable.

| Element | No-color cue |
|---|---|
| Heading 1–6 | bold + the **level shade glyph `█▓▒░▏·`** (forced on at Depth::None); density = level |
| Strong/Emphasis/StrongEmphasis | bold / italic / bold+italic |
| Code, CodeBlock | reverse |
| Link | underline |
| Diagnostics (spell/grammar) | **bold + underline** (distinct from a plain-underline Link) |
| Strikethrough | strike |
| BlockQuote | `▎` prefix glyph + indent |
| ThematicBreak | `───` glyph |
| ListMarker | bullet glyph |
| FrontMatter | reverse (inverted metadata block) |
| Comment | italic |
| **Selection** | **reverse + underline** (compound, so it stays visible *over* reverse elements like Code/FrontMatter/Search — Codex C3) |
| SearchMatch / SearchCurrent | reverse / reverse+bold |
| FoldMarker | `▸` + `… N lines` |
| FocusDim | inactive rows `DIM`; active region full-weight (contrast is the cue) |

**Enforced by tests, two layers:** (1) core: `no_color().face(el)` has ≥1 modifier
for every Face-cued element; (2) **shell render proof** — a `TestBackend` buffer in
**LivePreview** with No-color, asserting **every `SemanticElement`** (the §8.3
coverage table) is distinguishable by modifier/glyph, including all six heading
levels, Selection×{Code,Search}, the chrome faces, and source-mode selection.

**Modifier scarcity (accepted):** ~5 modifiers for ~20 elements, so several cues
reuse `reverse`; each element is individually distinguishable, and overlaps resolve
by pipeline order (Selection's compound `reverse+underline` keeps it visible over
other reverse elements). No element relies on color alone when color is off.

## 5. Config (extends §12.5 / 5a)
```toml
[theme]
name = "phosphor-amber"          # built-in; default = "default"
# file = "~/.config/wordcartel/base16-gruvbox-dark.yaml"   # OR a base16/24 palette
# depth = "truecolor"            # override detection (truecolor|256|16|none)
# heading_level_glyph = true     # override the theme's flag (ignored — forced on — at depth none)
[theme.styles]                   # per-element overrides
heading1  = { fg = "#bb9af7", bold = true }
selection = { bg = "#283457" }
```
- `RawThemeConfig{ name: Option<String>, file: Option<String>, depth: Option<String>,
  heading_level_glyph: Option<bool>, styles: BTreeMap<String, RawFace> }`;
  `RawFace` all-`Option` (omitted ≠ false).
- **Discriminated source across layers:** a layer setting `name` clears accumulated
  `file` (and vice-versa); both in one layer → `file` wins + warning.
- `~` expansion + `file` resolved relative to the declaring config file (provenance retained).
- `[theme.styles]` keys = snake-case element names; unknown → warning.
- serde `default` so pre-theming configs load unchanged.

## 6. Live switching
A `theme` command opens a **theme-picker overlay** (mirrors `outline_overlay`):
fuzzy list of `builtin_names()` + known names, Enter applies (session-only) +
relayouts, Esc cancels; XOR + non-key fallthrough (5e/5f lesson). No arg command
(handlers take no args). At effective depth `None`, applying keeps colors off.

## 7. Error handling
| Situation | Behavior |
|-----------|----------|
| Unknown name / unreadable/invalid base16 file | `default()` + warning |
| Bad hex / unknown key in `[theme.styles]` | skip that field/key + warning; never half-apply / crash |
| `NO_COLOR` / `TERM=dumb`/empty | effective depth `None` → `no_color()` (heading glyph forced on) |
| `name` + `file` in one layer | `file` wins + warning |
| Terminal lacks truecolor | `quantize` to detected depth |
| Default theme | reproduces today's pre-existing color sites exactly (§9 note) |

## 8. Testing

### 8.1 Core
- Every built-in **total** over `SemanticElement` (incl. `Heading(1..=6)`, clamp 0/7); phosphor family generates 10 total themes.
- `default()` faces match today's hardcoded sites; `default().base_*` = `Default` (source untouched). Golden anchor.
- `quantize` known Rgb → Indexed (cube+grays) and Ansi16; passthrough; idempotent.
- `from_base16` canonical mapping; total from a 16-slot input. Hand-rolled base16 parser: valid file, missing/extra slots, quotes/`#`, junk → error.
- `no_color()` all-`Default`, every Face-cued element ≥1 modifier. Phosphor-flat: all elements share `base_fg`, distinguished by modifiers; phosphor-shaded: distinct shades.
- `Style` total-map test (so `Style::Comment` can't miss an arm).
- **Producers:** blockquote/thematic-break prefix glyphs; inline `<!--x-->`→`Comment` but `<span>`→Plain; byte-0 `---` block→`FrontMatter` but mid-doc `--- ---`→**unchanged** (oracle full==incremental); block `<!--`→`HtmlComment`, `<div>`→`HtmlBlock`.

### 8.2 Shell
- `detect_depth` cases + precedence; `resolve_theme` (built-in/base16/bad-input/discriminated source/relative path).
- `face_to_ratatui` per depth; `compose` cross-products (heading+code, link+diag, selection×code, selection×search, focus+search).
- **Prefix geometry (§3.7):** cursor round-trips and mouse hit-test land correctly on rows with a list/blockquote/heading prefix, including wrapped + narrow-width rows (the regression net for the keystone).
- Selection painting (both modes; empty = no-op; wrapped; under search). Heading glyph (on for No-color/forced at depth none; off otherwise; never in source).
- Phosphor source tint: a phosphor theme tints source-mode cells with `base_fg/bg`; Default leaves source = terminal default.
- `theme` picker opens/applies/relayouts; depth `None` keeps colors off.

### 8.3 §13.2 coverage table (the a11y proof — Codex Part3-3)
A checked table keyed by **every** `SemanticElement` (Text, all inline, Heading 1–6,
BlockQuote, CodeBlock, ListMarker, ThematicBreak, FrontMatter, Comment, Selection,
SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar, FocusDim, FoldMarker,
WrapGuide, Chrome×3) → each has either a core modifier assertion **or** a concrete
No-color render fixture (LivePreview, and source mode where the element appears).
The build fails if an element lacks a row.

## 9. Performance & the "Default reproduces today" guarantee
- Resolution/depth/relayout happen **once** at startup and on switch — never per
  frame. `compose`/`quantize` are O(1) per span; base16 read is one-time shell IO.
- **Guarantee (narrowed, Codex Part3-2):** the Default theme reproduces **all
  pre-existing color/style sites exactly** — *except* the explicit scope-B additions
  (blockquote `▎` / thematic-break `───` glyphs, document-selection painting). Those
  are new and **update** their producer/render goldens intentionally (the existing
  "blockquote has no prefix glyph" integration expectation is revised, not preserved).

## 10. Risks & mitigations
| Risk | Mitigation |
|------|------------|
| Synthetic prefix desyncs caret (5g trap, ×3, latent in list bullets) | §3.7 width-accounted layout prefix; cursor/mouse round-trip tests |
| Global metadata option breaks incremental oracle | byte-0-only front-matter parser + reparse-from-0 on delimiter edits + oracle cases (§3.9) |
| `<div>` colored as comment / inline tag mis-styled | `HtmlComment`/`Comment` only when source begins `<!--`; inline only `<!-- -->` (§3.9) |
| Selection invisible over reverse elements | compound `reverse+underline` (§4) + overlap tests |
| Heading hierarchy lost at depth none | glyph forced on when `Depth::None` (§4) |
| `serde_yml` unmaintained/RUSTSEC | hand-rolled base16 parser, no YAML dep (§3.3) |
| SourcePlain over-styled | source modes apply base canvas + overlays only, no semantic faces (§3.5) |
| Centralizing 21 colors regresses look | Default reproduces pre-existing sites; golden tests (§9) |
| Effort too big for one plan | three independently-green plans (§12) |

## 11. Out of scope → future
- Helix/`.tmTheme`/VSCode importers; YAML-syntax highlighting inside front matter;
  full chrome re-skin (dialogs, scrollbar beyond base); per-buffer themes; theme
  hot-reload; theme-editor UI; richer diagnostic shape (curly underline/gutter);
  SourceHighlighted true syntax highlighting.

## 12. Execution — three independently-green plans (one design)
Codex flagged ~13–15 tasks across independent invariants as too big for one plan.
Split (each its own plan → green build/tests before the next):

1. **Theme model & centralization** — `theme.rs` (Color/Face/Theme/SemanticElement,
   quantize, built-ins incl. phosphor + shade ramp), `compose.rs` pipeline +
   `face_to_ratatui`, chrome face table, replace render's hardcoded colors with
   `compose`, Default golden no-regression. *No new producers/geometry yet.*
2. **Producers, geometry & §13.2** — the §3.7 cursor-safe prefix geometry
   (layout/ColMap/nav/mouse) + blockquote/thematic-break/heading glyphs; front-matter
   (byte-0) + comment (block/inline) producers; document-selection painting; the §8.3
   accessibility coverage proof. *The keystone plan; alters core layout + parser invariants.*
3. **Import, config & switching** — hand-rolled base16/24 import, `[theme]` config
   (RawFace, discriminated source, depth precedence/detection, `~`/relative paths),
   the theme-picker overlay + `theme` command + relayout-on-switch.
