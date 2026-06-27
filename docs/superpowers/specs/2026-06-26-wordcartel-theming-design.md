# Wordcartel — Markdown Theming — Design

**Date:** 2026-06-26
**Status:** Design (pre-plan) — Codex spec-reviewed (17 findings folded in)
**Effort:** Theming (standalone; after Effort 5 complete). **Closes** the v1 §13.2
accessibility item (color-independent legibility) and the §5-backlog "configurable
themes beyond the default." Scope **B**: the no-color path is fully cued — including
heading-level hierarchy (theme-controlled level glyph) and **document-selection
painting** (a pre-existing 5c gap, built here). ~13–15 tasks.
**Parent spec:** `docs/superpowers/specs/2026-06-21-wordcartel-design.md` (§13.2; §5 Backlog "Configurable themes"; §3.11 render modes; §13 construct set).
**Coverage ledger:** `docs/superpowers/plans/2026-06-22-wordcartel-coverage-ledger.md`

## 1. Summary

Markdown in wordcartel is nearly colorless: only inline `Style`
(emphasis/strong/code/link) gets color, **block roles (headings, blockquotes,
code blocks) get none**, and render.rs hardcodes ~21 scattered `Color::` literals
with no central palette.

This effort adds **themes**: a theme = **palette + a markdown-element→style
mapping**, modeled as pure data in `wordcartel-core`, resolved to ratatui styles
in the shell through a single composition seam. We ship hand-tuned built-ins
(**Default**, **No-color/high-contrast**, **Tokyo Night**), allow selection by
name or **any base16/base24 palette**, allow per-element overrides in config, and
**auto-downsample** to the terminal's color depth. The Default theme reproduces
today's exact look (golden-tested) — zero visible change unless opted in.

Two small **core producer additions** make more of markdown themable: **YAML
front matter** (enable metadata parsing → a `FrontMatter` role) and **comments**
(`<!-- -->`, block + inline → a `Comment` element). **Structural glyphs** give
elements a non-color cue: blockquote `▎` and thematic-break `───` (always on), a
**theme-controlled heading-level glyph** (a shade ramp `█▓▒░▏·` shown by the
No-color theme, off in colored themes since color carries the level), and
**document-selection painting** (the primary selection rendered on document cells —
new; today selection is read only for word-count).

Scope is **markdown text + the overlays painted on it** (search, diagnostics,
focus, **selection**) + a minimal **chrome palette** (so the Default theme
reproduces today's status/menu look, and other themes derive a coherent neutral
chrome).

## 2. Goals / Non-Goals

### Goals
- Pure `wordcartel-core::theme` (UI-agnostic; `#![forbid(unsafe_code)]`, IO-free).
- One shell composition seam producing a ratatui style from a stack of faces.
- Built-ins: **Default** (reproduces today), **No-color/high-contrast**, **Tokyo Night**.
- **base16/base24 palette import** (one canonical markdown mapping → hundreds of schemes).
- Per-element config overrides; auto color-depth degradation; live theme switch.
- **New themable producers:** front matter (block face) + comments (block & inline).
- **Full §13.2 (scope B):** blockquote + thematic-break glyphs; theme-controlled
  heading-level glyph (1–6 distinguishable without color); **document-selection
  painting**; diagnostics distinct from links without color.

### Non-Goals (v1)
- YAML-syntax highlighting *inside* front matter (keys/values) — block face only.
- Helix/`.tmTheme`/VSCode importers; full chrome re-skin beyond the small palette;
  per-buffer themes; theme hot-reload; a theme-editor UI; inline-image tinting.

## 3. Architecture

Functional-core / imperative-shell.

```
wordcartel-core (IO/thread-free, #![forbid(unsafe_code)])
  theme.rs    NEW — Color, Face, Theme, SemanticElement, built-ins, quantize,
                    BasePalette, from_base16. Pure (no ratatui).
  md_parse.rs ~  + blockquote prefix glyph "▎"; thematic-break glyph; inline
                    `<!-- -->` → Style::Comment span.
  block_tree.rs ~ + map metadata block → BlockRole::FrontMatter (enable parse opt);
                    HtmlBlock comment → role usable as Comment.
  style.rs    ~  + Style::Comment variant (inline).

wordcartel (shell)
  theme_load.rs NEW — ResolvedTheme { theme, depth, warnings }; detect_depth;
                      resolve_theme(cfg); base16/24 YAML parse; ~/relative paths.
  compose.rs    NEW — the face-composition pipeline + face_to_ratatui(depth) seam.
  theme_picker.rs NEW — overlay listing builtin/known themes (mirror outline_overlay).
  config.rs     + RawThemeConfig { name, file, depth, styles: BTreeMap<String,RawFace> }.
  render.rs     ~ replace style_to_ratatui + all hardcoded Color:: with compose(stack);
                    + paint document Selection (both render paths); + heading-level
                    shade glyph when theme.heading_level_glyph (live-preview, theme-side).
  registry.rs   + `theme` command → opens the theme picker.
  editor.rs/app.rs + active Theme + Depth seeded at startup; picker swaps Theme.
```

### 3.1 Core: `Color`, `Face`, `Theme`

```rust
/// A color in a theme. UI-agnostic; the shell maps it to ratatui.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Rgb { r: u8, g: u8, b: u8 }, // truecolor (themes' common case)
    Ansi16(u8),                  // 0..=15 named ANSI
    Indexed(u8),                 // 0..=255 palette index (the 256-color quantize target)
    Default,                     // terminal default fg/bg (no-color); == ratatui Color::Reset
}

/// One look. Every Option None = "inherit accumulated" in composition (§3.4);
/// `Some(Color::Default)` = explicitly reset to terminal default (distinct from None).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Face {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub underline_color: Option<Color>, // diagnostics color the UNDERLINE, not the text
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub reverse: Option<bool>,
}

/// Every themable element (v1). Inline + block roles + front-matter/comment +
/// on-text overlays + the minimal chrome palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SemanticElement {
    Text,                                            // base body
    Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link, // inline (map from core Style)
    Heading(u8),                                     // 1..=6 (clamped)
    BlockQuote, CodeBlock, ListMarker, ThematicBreak,
    FrontMatter, Comment,                            // NEW producers
    Selection,                                       // document selection (NEW painting; no-color = reverse)
    SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar, FocusDim, FoldMarker, WrapGuide,
    Chrome, ChromeSelected, ChromeMuted,             // status/menu/overlay frames + selected rows
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Theme {
    pub name: String,
    pub base_fg: Color,
    pub base_bg: Color,
    /// Show the heading-level shade glyph (█▓▒░▏·) in live-preview. The No-color
    /// theme sets this true (level cue without color); colored themes default
    /// false (color already conveys level — no prefix clutter). Overridable in config.
    pub heading_level_glyph: bool,
    faces: ThemeFaces, // total: a Face per element (private; via face())
}
impl Theme {
    pub fn face(&self, el: SemanticElement) -> Face; // total; Heading(n) clamps 1..=6
    pub fn builtin(name: &str) -> Option<Theme>;     // "default" | "no-color" | "tokyo-night"
    pub fn builtin_names() -> &'static [&'static str];
}
pub fn default() -> Theme;     // reproduces today (incl. search-yellow, diag red/blue underline, chrome b/w)
pub fn no_color() -> Theme;    // all fg/bg/underline_color = Default; cues via modifiers/glyphs (§4)
pub fn tokyo_night() -> Theme; // MIT palette from tokyonight.nvim
```

Notes: `face` is **total** — every theme has a Face for every element (no missing
element at a render site). Core has **no ratatui dependency**; the shell owns the
mapping (`Color::Default` → `ratatui::Color::Reset`).

### 3.2 Core: depth + quantize + base palette

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Depth { Truecolor, Indexed256, Ansi16, None }

/// Pure nearest-color downsample. Rgb→Indexed (6x6x6 cube + 24 grays) at 256;
/// Rgb/Indexed→Ansi16 at 16. Ansi16/Default pass through. Caller handles None
/// (forces no_color upstream — quantize is never asked for None).
pub fn quantize(c: Color, depth: Depth) -> Color;

/// A base16 (16 slots) or base24 (24 slots) palette — the base24 extra 8 optional.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BasePalette { pub base: [Color; 16], pub extra: Option<[Color; 8]> }

/// THE canonical markdown mapping over base palette slots → a full Theme.
/// base00 bg · base05 text · base03 muted/comment/frontmatter · base0D headings+links ·
/// base0B code(green) · base0E emphasis(purple) · base09 strong · base08 diag-spell ·
/// base0A diag-grammar+search · base24 extras (if present) refine diag/search. (Exact
/// table fixed in the plan.) Always yields a TOTAL theme even from a 16-slot input.
pub fn from_base16(name: &str, p: BasePalette) -> Theme;
```

### 3.3 Shell: resolution, depth detection, the seam

`theme_load.rs`:
- `detect_depth() -> Depth` (case-insensitive): `NO_COLOR` set → `None`; `TERM`
  empty or `dumb` → `None`; `COLORTERM` ∈ {`truecolor`,`24bit`} → `Truecolor`;
  `TERM` contains `-direct` → `Truecolor`; `TERM` contains `256color` →
  `Indexed256`; else `Ansi16`.
- **Depth precedence (locked): `NO_COLOR` > explicit `[theme] depth` > detection.**
  The forced/effective depth is stored separately from the selected theme; when
  effective depth is `None`, the picker/`theme` command cannot re-enable color.
- `resolve_theme(cfg) -> ResolvedTheme { theme, depth, warnings }`:
  1. effective depth per the precedence above; if `None` → theme = `no_color()`.
  2. else base = `builtin(cfg.name)` OR `from_base16(parse(cfg.file)?)`; error → `default()` + warning.
  3. apply `cfg.styles` per-element overrides (merge each present `Face` field; bad hex/unknown key → skip + warning).
  4. `warnings` are appended to the existing startup warning stream (`config.rs` already returns `(Config, Vec<String>)`).

`compose.rs` — the seam (replaces `style_to_ratatui` + the 21 hardcoded colors):
- `face_to_ratatui(face: Face, depth: Depth) -> ratatui::Style` — quantize each
  color to `depth`; `Color::Default`→`Reset`; map the 5 modifiers + underline_color.
- `compose(theme, depth, stack: &[SemanticElement]) -> ratatui::Style` — fold the
  faces in order, each present field overriding the accumulator (§3.4).

### 3.4 Composition pipeline (the rule render follows)

A painted glyph's style is the ordered patch of faces (each `Some` field wins;
`None` preserves the accumulator; `Some(Color::Default)` hard-resets that color):

```
Text(base)  →  block role (Heading/BlockQuote/CodeBlock/FrontMatter/Comment/ThematicBreak)
            →  inline style (Emphasis/Strong/Code/Link/Strikethrough)
            →  FocusDim (if row inactive)
            →  Selection (if the glyph is inside the primary selection range)
            →  SearchMatch / SearchCurrent (if the glyph is in a match)
            →  DiagSpelling / DiagGrammar (stacks underline + underline_color; does NOT change fg)
```

**Document selection** is projected onto visible glyphs in **both** render paths
(live-preview and source) from `editor.active().document.selection.primary()` —
mapping the byte range to cells via the same ColMap render already uses for
search. Selection sits below search so a search-current match still stands out
while selecting. No-color `Selection` face = reverse.

So a heading containing inline `code` = heading face then code face; a link with a
spelling diagnostic = link underline + diagnostic underline_color; search on a
strong word = strong then search bg. `ListMarker` applies only to the row's
`prefix_glyph`; `FoldMarker`/`WrapGuide` to their own glyph spans. The pipeline is
deterministic and unit-tested for the cross-products in §8.

### 3.5 Render mode behavior (§3.11 interaction)

- **LivePreview:** full pipeline (roles + inline + overlays + structural glyphs +
  the heading-level glyph when the theme enables it).
- **SourceHighlighted / SourcePlain:** both collapse to `source_mode` today (raw
  text, no inline styles). This effort applies **`Text` base color + overlays
  (Selection/search/diag/focus)** in source modes — roles/inline are NOT recolored,
  and the heading-level glyph is **not** added (the literal `#` markers are already
  visible). `Selection` paints in every mode (it's a cursor concern, not a
  conceal concern). Making SourceHighlighted truly syntax-highlight source is **out
  of scope** (documented; it currently equals SourcePlain).

### 3.6 Active theme location
`Editor` gains `theme: Theme` + `depth: Depth`, seeded at startup (like
`view_opts`/`diag_cfg`). render borrows them read-only. The theme picker swaps
`editor.theme` in place (session-only; config persists). Resolution happens once
at startup and on switch — never per frame.

## 4. §13.2 — color-independent cues (scope B: fully closed)

**Every themed element carries a non-color cue** so
meaning survives color-stripping. The **No-color theme is the colors-stripped
derivation** (all fg/bg/underline_color → `Default`, modifiers/glyphs kept) — it
is the proof object.

| Element | No-color cue |
|---|---|
| Heading 1–6 | bold **+ the theme's level shade glyph `█▓▒░▏·`** (No-color sets `heading_level_glyph = true`); the glyph density makes all six levels distinguishable without color |
| Strong / Emphasis / StrongEmphasis | bold / italic / bold+italic |
| Code, CodeBlock | reverse |
| Link | underline |
| **Diagnostics (spell/grammar)** | **bold + underline** (distinct from a plain-underline Link — fixes the link/diag collision) |
| Strikethrough | strike |
| BlockQuote | **`▎` prefix glyph + indent** (NEW glyph) |
| ThematicBreak | **`───` glyph** (NEW; was concealed to nothing) |
| ListMarker | bullet glyph (existing) |
| FrontMatter | reverse (the metadata block reads as an inverted region) |
| Comment | italic (muted prose convention; color is secondary) |
| **Selection** | **reverse** (painted on document cells in both render paths) |
| SearchMatch / SearchCurrent | reverse / reverse+bold |
| FoldMarker | `▸` + `… N lines` (existing) |
| FocusDim | inactive rows get `DIM`; the active region stays full-weight (contrast is the cue) |

**Enforced by tests** — not a core allow-list alone (Codex: a core-only check
passes while live-preview lacks the glyph). Two layers:
1. Core: `no_color().face(el)` has ≥1 modifier set for every element that relies on
   a Face cue.
2. **Shell render tests** (the real proof): render a doc exercising every element in
   **LivePreview** with the No-color theme into a `TestBackend` buffer and assert
   each is distinguishable by modifier/glyph — a blockquote row shows `▎`; a
   thematic break shows `───`; **headings 1–6 show distinct shade glyphs `█▓▒░▏·`**;
   a diagnostic word is bold+underline vs a link's plain underline; **a selected
   span is reverse**; front-matter reverse; comment italic. This catches a missing
   producer glyph, which a core test can't.

**Modifier scarcity (accepted):** terminals offer ~5 modifiers
(bold/italic/underline/reverse/strike) for ~20 elements, so several no-color cues
reuse `reverse` (Selection, SearchMatch, Code, FrontMatter). Each element is
individually distinguishable (the §4-layer-2 render proof tests each in
isolation); **overlapping** reverse elements (e.g. selecting inline code) resolve
by pipeline order (§3.4) — the later, more-transient face wins. This is the
realistic §13.2 bar: every distinction has a non-color cue; not every *pair* is
separable under simultaneous overlap. No element relies on color alone.

## 5. Config (extends §12.5 / 5a)

```toml
[theme]
name = "tokyo-night"            # built-in name; default = "default"
# file = "~/.config/wordcartel/base16-gruvbox-dark.yaml"  # OR a base16/24 palette
# depth = "truecolor"          # override auto-detect (truecolor|256|16|none)
# heading_level_glyph = true   # override the theme's level-glyph flag
[theme.styles]                 # optional per-element overrides
heading1  = { fg = "#bb9af7", bold = true }
link      = { fg = "#7aa2f7", underline = true }
comment   = { fg = "#565f89", italic = true }
selection = { bg = "#283457" }
```

- Raw types: `RawThemeConfig { name: Option<String>, file: Option<String>,
  depth: Option<String>, heading_level_glyph: Option<bool>,
  styles: BTreeMap<String, RawFace> }`; `RawFace` has all `Option<String>` colors
  + `Option<bool>` modifiers (so "omitted" ≠ "false").
- **Discriminated source across layers (Codex F13):** when a config layer sets
  `name`, it CLEARS the accumulated `file` (and vice-versa); both in one layer →
  `file` wins + warning. Prevents a low-layer `file` from shadowing a high-layer `name`.
- Layers on built-in < XDG < project-local (5a), per field.
- `~` expansion + **`file` resolved relative to the declaring config file** (layer
  provenance retained through the merge).
- `[theme.styles]` keys are snake-case element names (`heading1`..`heading6`,
  `code`, `code_block`, `block_quote`, `front_matter`, `comment`, `link`,
  `selection`, `search_match`, `diag_spelling`, `diag_grammar`, `chrome`, …);
  unknown → warning.
- **base16/24 file format:** the standard tinted-theming YAML (`scheme`, `author`,
  `base00`..`base0F`, optional `base10`..`base17`). Parsed in the **shell** via a
  YAML dep (`serde_yml`, the maintained serde_yaml fork) — core stays IO-free.
- serde `default` so pre-theming configs load unchanged.

## 6. Live switching
A `theme` command opens a **theme-picker overlay** (`theme_picker.rs`, mirrors
`outline_overlay`): fuzzy list of `builtin_names()` + any `[theme]`-known name,
Enter applies to `editor.theme` (session-only), Esc cancels; XOR with the other
overlays + non-key fallthrough (the 5e/5f lesson). Registry handlers take no args,
so selection is via the overlay, not a `set-theme <name>` arg command. When
effective depth is `None`, the picker shows but applying keeps colors off.

## 7. Error handling / edge cases

| Situation | Behavior |
|-----------|----------|
| Unknown `[theme] name` | `default()` + startup warning |
| Unreadable / invalid base16 file | `default()` + warning |
| Invalid hex / unknown key in `[theme.styles]` | skip that field/key + warning; never half-apply / crash |
| `NO_COLOR` set | effective depth `None` → `no_color()`, regardless of name/depth |
| `name` + `file` both in one layer | `file` wins + warning |
| Terminal lacks truecolor | `quantize` to detected depth |
| `TERM=dumb`/empty | depth `None` (no color) |
| Default theme | reproduces today's exact look (golden-tested) |

## 8. Testing

### 8.1 Core (`theme`, `md_parse`, `block_tree`)
- Every built-in is **total** over `SemanticElement` (incl. `Heading(1..=6)`, clamp 0/7).
- `default()` faces match today's hardcoded look (Code=Cyan, Link=Yellow+underline,
  Strong=bold, Emphasis=italic, search=yellow-bg, diag spell/grammar underline_color
  red/blue, chrome black/white) — the golden anchor.
- `quantize`: known Rgb → expected Indexed (cube + gray ramp) and → expected Ansi16;
  Ansi16/Default pass through; idempotent per depth.
- `from_base16`: canonical mapping; a 16-slot (no `extra`) input still yields a total theme.
- `no_color()`: no `Rgb`/`Ansi16`/`Indexed` on any face (all `Default`); every Face-cued element has ≥1 modifier (§4 layer 1).
- **Producers:** blockquote analysis emits `▎` prefix glyph; thematic break emits `───`;
  inline `<!-- x -->` → `Style::Comment` span; a metadata block parses to `BlockRole::FrontMatter`; a block `<!-- -->` is reachable as `Comment`.

### 8.2 Shell
- `detect_depth`: `NO_COLOR`→None; `dumb`/empty→None; `COLORTERM=truecolor` (any case)→Truecolor; `*-direct`→Truecolor; `*256color*`→256; else Ansi16; `[theme] depth` override honored; precedence `NO_COLOR > depth > detect`.
- `resolve_theme`: name→built-in; bad name→default+warning; base16 file→theme; bad file→default+warning; `[theme.styles]` per-field merge; bad hex→skip+warning; discriminated `name`/`file` clearing across layers; `~`/relative path.
- `face_to_ratatui` per depth (Rgb passthrough at Truecolor; Indexed at 256; Ansi16 at 16; Default→Reset; underline_color mapped).
- **`compose` pipeline:** heading+strong, heading+code, link+diagnostic, focus+search, search+diagnostic — correct precedence in LivePreview.
- **§13.2 render proof (§4 layer 2):** No-color LivePreview buffer — blockquote `▎`, thematic-break `───`, diagnostic bold+underline vs link underline-only, front-matter reverse, comment italic — each asserted on `TestBackend` cells.
- **Golden no-regression:** Default theme → existing render tests unchanged.
- **New coloring:** a Tokyo Night heading row's cells carry the heading fg; a front-matter block carries the front-matter face; a comment carries the comment face.
- **Selection painting:** a non-empty selection paints the selected document cells (reverse in No-color; the Selection face fg/bg in colored themes) in BOTH live-preview and source modes; an empty selection paints nothing; selection maps correctly across a wrapped line via ColMap; composes under search (a search-current match still distinct over a selection).
- **Heading-level glyph:** with `heading_level_glyph=true` (No-color) a heading row shows the level's shade glyph (`█` for h1 … `·` for h6) in live-preview; with it false (Default/Tokyo Night) no glyph is added; never added in source modes; config can override the flag.
- `theme` command opens the picker; selecting a theme repaints with it; depth `None` keeps colors off.

## 9. Performance / responsiveness (#1 priority)
Theme resolution (built-in lookup or base16 parse + override merge) and depth
detection happen **once** at startup and on switch — never per frame/keystroke.
`compose`/`quantize` are O(faces-in-stack) ≈ O(1) per span, same order as today's
`style_to_ratatui`. base16 file read is one-time startup IO in the shell; core
stays IO-free. No new thread/channel on the hot path.

## 10. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| Centralizing 21 colors regresses the look | Default reproduces today; golden render tests gate it |
| Composition order wrong (heading+code, link+diag) | one documented pipeline (§3.4) + cross-product tests |
| §13.2 "proof" passes but a live-preview glyph is missing | the proof is a **render** test in LivePreview, not a core allow-list (Codex F7) |
| `Color` can't express 256 / Base24 input | `Indexed(u8)` + `BasePalette{base,extra}` (Codex F5/F10) |
| Front-matter/comment have no producer | this effort ADDS them (parser opt + role map + md_parse comment span) (reverses Codex F16) |
| Truecolor theme on a weak terminal | depth precedence + `quantize` + `NO_COLOR`/dumb → no-color |
| Config `file` shadows a higher-layer `name` | discriminated source clears the other per layer (Codex F13) |
| set-theme can't be an arg command | theme-picker overlay instead (Codex F17) |
| Effort is large (scope B, ~13–15 tasks) | the plan sequences it: core types/model first, then producers, render centralization + composition, selection painting, heading-level glyph, base16, config, depth, picker, accessibility proof — each an independently testable task |
| Selection painting (new render feature) interacts with wrap/ColMap | reuse the exact ColMap byte→cell projection render already uses for search highlights; test across a wrapped line |
| Heading-level glyph clutters colored themes | theme-controlled flag (default off in colored themes, on in No-color); config-overridable |

## 11. §13.2 closure note (scope B — all in this effort)
The three items that fully close §13.2 are **in scope** here:
- **Document-selection painting** — render the primary selection on document cells
  (a real missing feature since 5c; today selection is read only for word-count),
  via the `Selection` element + reverse no-color face, both render paths (§3.4/§3.5).
- **Heading-level hierarchy without color** — the theme-controlled shade glyph
  (`heading_level_glyph`), so headings 1–6 are distinguishable in No-color (§4).
- **Link vs diagnostic** — diagnostics are **bold+underline**, links plain underline (§4).

A *richer* diagnostics shape (curly underline / gutter) and SourceHighlighted
syntax highlighting remain genuinely future (§12) — they are quality refinements,
not §13.2 gaps.

## 12. Out of scope → future
- Helix `markup.*` / `.tmTheme` / VSCode importers.
- YAML-syntax highlighting inside front matter (keys/values) — block face only here.
- Full chrome re-skin (dialogs, scrollbar styling beyond base); per-buffer themes;
  theme hot-reload; a theme-editor / live color-picker UI.
