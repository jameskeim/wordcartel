# Wordcartel — Markdown Theming — Design

**Date:** 2026-06-26
**Status:** Design (pre-plan)
**Effort:** Theming (standalone; after Effort 5 complete). Closes the v1 §13.2 accessibility gap and the §5-backlog "configurable themes beyond the default."
**Parent spec:** `docs/superpowers/specs/2026-06-21-wordcartel-design.md` (§13.2 no-color/high-contrast; §5 Backlog "Configurable themes"; §3.11 render modes are orthogonal — a theme colors any mode).
**Coverage ledger:** `docs/superpowers/plans/2026-06-22-wordcartel-coverage-ledger.md`

## 1. Summary

Markdown in wordcartel is currently nearly colorless: only inline `Style`
(emphasis/strong/code/link) gets any color, and **block roles (headings,
blockquotes, code blocks) get none** — render.rs hardcodes ~21 scattered
`Color::` literals and a `style_to_ratatui` table, with no central palette.

This effort adds **themes**: a theme is a **palette + a markdown-element→style
mapping**, modeled as pure data in `wordcartel-core`, resolved to ratatui styles
in the shell through a single seam. We ship hand-tuned built-ins (**Default**,
**No-color/high-contrast**, **Tokyo Night**), let a user select by name or bring
**any base16/base24 palette**, allow per-element overrides in config, and
**auto-downsample** to the terminal's color depth. The No-color built-in closes
the v1 §13.2 accessibility requirement; the Default built-in reproduces today's
look so nothing changes unless the user opts in.

Scope is **markdown text + the overlays painted on it** (search highlight,
diagnostic underlines, focus-dim, fold marker, wrap guide). Structural chrome
(status bar, menu, palette frame) stays neutral, borrowing only the theme's base
fg/bg.

## 2. Goals / Non-Goals

### Goals
- A pure `wordcartel-core::theme` data model (UI-agnostic; `#![forbid(unsafe_code)]`, IO-free).
- A single shell seam `face_to_ratatui` replacing `style_to_ratatui` + the 21 hardcoded colors.
- Three built-ins: **Default** (reproduces today's colors), **No-color/high-contrast** (§13.2), **Tokyo Night**.
- **base16/base24 palette import** (`from_base16`) — one canonical markdown mapping turns hundreds of published schemes into themes.
- Per-element overrides in config (`[theme.styles]`), layered on the chosen theme.
- **Automatic color-depth degradation** (Truecolor / 256 / 16 / None) via a pure `quantize`.
- The **§13.2 invariant** enforced by test: no semantic distinction relies on color alone.
- Live `set-theme` command + palette entries (session switch); config persists.

### Non-Goals (v1)
- Helix `markup.*` / Sublime `.tmTheme` / VSCode importers (a follow-up — base16 covers breadth now).
- Full chrome re-skin (status bar / menu / palette frame stay neutral; they borrow base fg/bg only).
- Per-buffer themes; theme hot-reload on file change; background images; animated/decorative color.
- Changing the `Style`/`BlockRole` semantic model in core (render already has the role + inline styles).

## 3. Architecture

Functional-core / imperative-shell.

```
wordcartel-core (IO/thread-free, #![forbid(unsafe_code)])
  theme.rs  NEW — Color, Face, Theme, SemanticElement, built-ins, quantize, from_base16. Pure.

wordcartel (shell)
  theme_load.rs  NEW — resolve active Theme from config; capability detection (Depth);
                       face_to_ratatui(face, depth) seam; bad-input fallbacks.
  config.rs      + ThemeConfig { name, file, styles } under [theme].
  render.rs      ~ replace style_to_ratatui + all hardcoded Color:: with theme.face(el)→face_to_ratatui.
  registry.rs    + `set_theme` command (+ a theme palette/listing entry).
  app.rs/editor.rs + active Theme + Depth seeded at startup (like view_opts/diag_cfg); set_theme swaps it.
```

### 3.1 Core: `wordcartel-core::theme`

```rust
/// A color in a theme. UI-agnostic; the shell maps it to a ratatui Color.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Color {
    Rgb { r: u8, g: u8, b: u8 },  // truecolor (the common case for themes)
    Ansi(u8),                     // 0..=15 named ANSI (for 16-color / hand-tuned)
    Default,                      // the terminal's own default fg/bg (no-color)
}

/// One resolved look for a semantic element. fg/bg None = inherit base / terminal default.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Face {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub reverse: bool,
}

/// Every themable element. Inline Style + BlockRole + the on-text overlays.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SemanticElement {
    // base
    Text, // base body text (base_fg on base_bg)
    // inline (mirror core::style::Style)
    Emphasis, Strong, StrongEmphasis, CodeInline, Strikethrough, Link,
    // block roles (mirror core::style::BlockRole)
    Heading(u8), // 1..=6
    BlockQuote, CodeBlock, ListMarker, ThematicBreak, FrontMatter,
    // markdown punctuation concealed/dimmed in live-preview
    ConcealedMarker,
    // overlays painted on the text
    SearchMatch, SearchCurrent, DiagSpelling, DiagGrammar,
    FocusDim, FoldMarker, WrapGuide,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Theme {
    pub name: String,
    pub base_fg: Color,
    pub base_bg: Color,
    faces: ThemeFaces, // a Face per SemanticElement (private; resolved via face())
}

impl Theme {
    /// The Face for an element. Heading(n) clamps n to 1..=6. Total — every
    /// element always resolves (a theme cannot have a "missing" element).
    pub fn face(&self, el: SemanticElement) -> Face;
    pub fn builtin(name: &str) -> Option<Theme>; // "default" | "no-color" | "tokyo-night"
    pub fn builtin_names() -> &'static [&'static str];
}

/// Built-ins (each total over SemanticElement).
pub fn default() -> Theme;     // reproduces today's colors (Code=Cyan, Link=Yellow+underline, …)
pub fn no_color() -> Theme;    // all fg/bg = Default; meaning carried by modifiers/glyphs (§4)
pub fn tokyo_night() -> Theme; // MIT palette from tokyonight.nvim

/// Terminal color depth.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Depth { Truecolor, Indexed256, Ansi16, None }

/// Pure nearest-color downsample. Rgb→256 (6x6x6 cube + grays) and 256/Rgb→16.
/// Ansi/Default pass through unchanged. `None` is handled by the caller (forces no-color).
pub fn quantize(c: Color, depth: Depth) -> Color;

/// Base16/Base24 palette (16 or 24 hex slots). base00=bg … base07=fg, base08..0F accents.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Base16 { pub slots: [Color; 16] } // base24's extra 8 optional, mapped if present

/// The ONE canonical markdown mapping over base16 slots → a full Theme.
/// e.g. base0D→headings/links, base0B→code(strings-green), base0E→emphasis(purple),
/// base08→errors/diag, base03→concealed/dim, base05→text, base00→bg. Documented in the plan.
pub fn from_base16(name: &str, p: Base16) -> Theme;
```

**Notes**
- `Theme::face` is **total**: a built-in or imported theme always has a Face for
  every element (no `Option`), so render never has to handle a missing element.
  Imported/partial inputs are completed by falling back to a base Face (Text).
- Core has **no ratatui dependency** — `Color`/`Face` are plain data. The shell
  owns the ratatui mapping. This keeps the theme model unit-testable in core.

### 3.2 Shell: resolution, depth, the seam

`theme_load.rs`:
- `detect_depth() -> Depth` from env: `NO_COLOR` set → `None`; `COLORTERM` in
  {`truecolor`,`24bit`} → `Truecolor`; `TERM` containing `256color` →
  `Indexed256`; else `Ansi16`. (Overridable by config `[theme] depth = "..."` for
  testing / forcing.)
- `resolve_theme(cfg: &ThemeConfig) -> Theme`:
  1. base = `Theme::builtin(cfg.name)` OR `from_base16(parse(cfg.file)?)`; on
     error → `default()` + a status warning (never crash).
  2. apply `cfg.styles` per-element overrides (merge each `Face` field; invalid
     hex → skip that field + warning).
- `face_to_ratatui(face: Face, depth: Depth) -> ratatui::style::Style` — the
  single seam: quantize each color to `depth`, map modifiers
  (bold/italic/underline/strike/reverse) to ratatui `Modifier`, map
  `Color::Default` to "no fg/bg set". `depth == None` is never reached here
  (resolution forced `no_color()` upstream), but the mapping still treats
  `Default` as unset.

`render.rs` integration (the centralization):
- Replace `style_to_ratatui(s: Style)` with `theme.face(style_element(s))` →
  `face_to_ratatui(.., depth)`.
- Replace each of the ~21 hardcoded `Color::` sites with the matching element:
  - heading rows → `Heading(level)` (NEW — roles get color for the first time),
  - concealed/dim markers (`DarkGray`) → `ConcealedMarker`,
  - focus-dim (`DarkGray`) → `FocusDim`,
  - search match (`Yellow` bg) → `SearchMatch`; current → `SearchCurrent`,
  - diagnostics underline (`Red`/`Blue`) → `DiagSpelling`/`DiagGrammar`,
  - fold marker (`DarkGray`) → `FoldMarker`,
  - wrap guide (`DarkGray`) → `WrapGuide`.
- Status bar / menu / palette frame: **not** themed per-element; they read
  `theme.base_fg`/`base_bg` only (neutral chrome). Search/menu REVERSED
  highlights stay as modifiers (already color-independent).

### 3.3 Where the active theme lives

`Editor` gains `theme: Theme` and `depth: Depth`, seeded at startup from config
(mirroring how `view_opts` / `diag_cfg` / `dictionary` are seeded). `render`
borrows `editor.theme`. The `set_theme` command swaps `editor.theme` in place
(session-only; config is the persistent source). No per-frame theme work — theme
resolution happens once at startup and on `set_theme`.

## 4. The §13.2 accessibility invariant (load-bearing)

**No semantic distinction may rely on color alone.** Every `SemanticElement`'s
Face carries a non-color cue so meaning survives with color stripped (colorblind
users, `NO_COLOR`, monochrome terminals).

- The **No-color theme is derived by stripping all fg/bg to `Color::Default`
  while keeping modifiers/glyphs.** It is therefore the canonical proof: if
  No-color is still fully legible, the cue set is sufficient.
- Cue assignment in No-color:
  - Heading 1 → bold + reverse; Heading 2 → bold + underline; Heading 3–6 → bold
    (depth conveyed by the existing source-mode markers / indent + weight).
  - Strong → bold; Emphasis → italic; StrongEmphasis → bold + italic.
  - CodeInline / CodeBlock → reverse (a block of inverted text reads as code).
  - Link → underline; Strikethrough → strike.
  - BlockQuote → `▎` prefix glyph (already a role glyph) + the quote indent.
  - ListMarker / ThematicBreak / FrontMatter → their existing glyphs.
  - SearchMatch → reverse; SearchCurrent → reverse + bold.
  - DiagSpelling / DiagGrammar → underline (shape distinguishes from link via
    context + the quick-fix affordance); color is the *secondary* cue only.
  - FoldMarker → the `▸` glyph + `… N lines` text (already non-color).
  - FocusDim → the existing DIM modifier (no color needed).
  - ConcealedMarker → DIM (or simply hidden in live-preview).
- **Enforced by a core test** `no_color_has_a_noncolor_cue_for_every_element`:
  for each `SemanticElement`, `no_color().face(el)` must have at least one of
  {bold, italic, underline, strike, reverse} set OR be an element whose
  distinction is structural (a documented allow-list: `Text`, `FoldMarker`,
  `ListMarker`, `ThematicBreak`, `FrontMatter`, `BlockQuote` carry a glyph/indent
  cue outside the Face). The allow-list is explicit so a regression that drops a
  cue fails the test.

This makes §13.2 a property of the theme **system**, not just one theme: any
colored theme, stripped of color, must still pass — so colored themes are
authored by *adding* color on top of an already-cue-bearing base.

## 5. Config & built-ins (extends §12.5 / 5a)

```toml
[theme]
name = "tokyo-night"          # a built-in name; default = "default"
# file = "~/.config/wordcartel/base16-gruvbox-dark.yaml"  # OR a base16/24 palette file
# depth = "truecolor"         # optional override of auto-detection (truecolor|256|16|none)

[theme.styles]                # optional per-element overrides, applied on top
heading1 = { fg = "#bb9af7", bold = true }
link     = { fg = "#7aa2f7", underline = true }
code     = { fg = "#9ece6a" }
```

- Layers on the existing built-in < XDG < project-local precedence (5a):
  the deepest `[theme]` wins per field, like other config sections.
- `name` and `file` are mutually exclusive; if both set, `file` wins + a warning.
- Override keys are the snake-case element names (`heading1`..`heading6`,
  `emphasis`, `strong`, `code`, `code_block`, `link`, `blockquote`,
  `search_match`, `diag_spelling`, …). Unknown keys → warning, ignored.
- A `set-theme <name>` command + a palette listing (`builtin_names()`) let the
  user try themes live; the change is session-only (config persists).

## 6. Error handling / edge cases

| Situation | Behavior |
|-----------|----------|
| Unknown `[theme] name` | fall back to `default()` + status warning |
| Unreadable / invalid base16 `file` | `default()` + warning |
| Invalid hex / unknown key in `[theme.styles]` | skip that one field/key + warning; never half-apply, never crash |
| `NO_COLOR` env set | force `no_color()` regardless of `name` (env wins; honors the convention) |
| Terminal lacks truecolor | `quantize` downsamples to detected depth; theme still renders |
| `set-theme` to an unknown name | status warning, theme unchanged |
| Default theme | reproduces today's exact colors (golden-tested) — zero visible change unless opted in |

## 7. Performance / responsiveness (#1 priority)

- Theme resolution (built-in lookup or base16 parse + override merge) happens
  **once at startup** and on `set-theme` — never per frame, never per keystroke.
- `face_to_ratatui` + `quantize` are O(1) per painted span (same order as the
  current `style_to_ratatui`); `quantize` is a handful of arithmetic ops.
- No new thread/channel/IO on the hot path. base16 file read is a one-time
  startup IO in the shell (core stays IO-free).

## 8. Testing

### 8.1 Core (`theme`)
- Each built-in is **total**: `face(el)` returns for every `SemanticElement`
  (incl. `Heading(1..=6)` and clamping `Heading(0)`/`Heading(7)`).
- `default()` Faces match today's hardcoded look (Code=Cyan, Link=Yellow+underline,
  Strong=bold, Emphasis=italic, dim=DarkGray-equivalents) — the golden anchor.
- `quantize`: known RGB → expected 256-cube index and → expected ANSI-16; grays
  to the gray ramp; `Ansi`/`Default` pass through; idempotent at each depth.
- `from_base16`: the canonical mapping assigns the documented slots; a partial
  (base16 vs base24) input still yields a total theme.
- **§13.2 invariant:** `no_color_has_a_noncolor_cue_for_every_element` (§4).
- `no_color()` has **no** `Rgb`/`Ansi` fg/bg on any element (all `Default`).

### 8.2 Shell
- `detect_depth`: `NO_COLOR`→None; `COLORTERM=truecolor`→Truecolor; `TERM=*256color*`→256; else Ansi16; config `depth` override wins.
- `resolve_theme`: name→built-in; bad name→default+warning; base16 file→theme; bad file→default+warning; `[theme.styles]` merges per field; bad hex→skipped+warning; `NO_COLOR` forces no_color.
- `face_to_ratatui`: each depth maps colors correctly (Truecolor passes Rgb; 16 quantizes; Default→unset); modifiers map to ratatui `Modifier`.
- **Render integration:** a heading row carries the active theme's heading fg
  (NEW behavior — assert a Tokyo Night heading row's cells have the heading
  color); No-color paints **no** fg but keeps bold on a heading; **golden
  no-regression:** with the Default theme, existing render tests are unchanged.
- `set_theme` command swaps the active theme and a subsequent render reflects it.

## 9. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| Centralizing 21 hardcoded colors regresses the current look | Default theme reproduces today's colors exactly; golden render tests gate it |
| Color-only distinctions slip in (a11y regression) | §13.2 invariant is a core test over the whole element set; colored themes build on a cue-bearing base |
| Truecolor theme looks wrong on limited terminals | auto depth-detect + pure `quantize`; `NO_COLOR`/no-color fallback; depth override for forcing |
| base16 mapping looks off for some schemes | one well-documented canonical mapping; per-element `[theme.styles]` overrides let a user fix any slot |
| Theme work bleeds into re-skinning all chrome | scope locked to text + on-text overlays; chrome borrows base fg/bg only (§2 non-goal) |
| Render borrow churn (theme read mid-row-loop) | `editor.theme` is an immutable borrow like `view_opts`; resolved once, read-only in render |

## 10. Out of scope → future
- Helix `markup.*` / `.tmTheme` / VSCode importers (faithful editor themes).
- Full chrome theming (status/menu/palette frame, dialogs).
- Per-buffer themes; theme hot-reload on config-file change; inline-image tinting.
- A theme *editor* / live color picker UI.
