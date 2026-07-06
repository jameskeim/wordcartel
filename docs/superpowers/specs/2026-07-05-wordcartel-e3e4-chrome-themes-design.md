# E3+E4 — derived chrome ladder, full|zen axis, theme lineup restructure (design)

Status: DRAFT (user-approved design 2026-07-05; seven forks resolved one at a time)
Effort: E3 (chrome theming coherence + the render.rs split, per the backlog) + E4 (bundled
themes) as ONE effort, E4's research executed first (deliverables:
`.superpowers/sdd/e4-themes-research.md` + the deep-research findings summarized in
Grounded facts). Working order: the E3 slot.

## Goals

1. **One chrome system, derived:** chrome faces are COMPUTED from each theme's base colors
   as a small tonal ladder (the research consensus: Material elevation overlays, Atlassian
   surface levels, Sublime Adaptive's generated UI); themes may override any face.
2. **Every cell themed:** the unthemed overlay interiors (`RStyle::default()` rows/query
   lines/backdrop gaps) and the modifier-only status bar die; every chrome surface draws
   from a named face under every full-color theme.
3. **A `full | zen` disposition axis** (config + a live toggle + persistence): zen
   collapses the ladder toward the canvas — the principled generalization of tokyo-night's
   subtle dark menus.
4. **Theme lineup restructure:** phosphor `-flat` removed; `default` renamed
   `terminal-plain`; a new `terminal-ansi` full-markdown-color theme in named ANSI colors;
   TEN new base16-derived builtins (user-picked bundle); **`flexoki-dark` becomes the
   launch default**.
5. **The render.rs split rides along** (backlog: E3 touches every overlay painter anyway).

## Non-goals

- E1's chrome/density preset umbrella, E2 checkable/radio menu items, A3 curation.
- Focus-dim / typewriter (writing modes, not chrome).
- No theme-picker redesign (18+ rows already handled by A6 windowing); no config
  `include`; no OSC 12 cursor-color control (deferred note).
- `terminal-plain` (ex-default) and `no-color` keep their identities (user-ratified),
  stated precisely (Codex r1 I-6): terminal-plain's CANVAS stays terminal-inherited
  (base fg/bg = Default — never an opinionated page color) while its CHROME keeps
  today's legacy named-ANSI family (White/Black bars, DarkGray muted — the pre-theming
  look is its contract); no-color is pure-modifier everywhere. Both are exempt from
  derivation by the Rgb-bases rule (D1).
- `theme::default()` — the CODE fn — keeps returning the plain table (renamed in `name`
  only): error-path fallbacks stay minimal-safe and the existing render-test corpus keeps
  its meaning. Only `resolve_theme`'s no-config arm changes to the launch default.

## Grounded facts

### Code (Explore map at 8b15f3d; fold-verified at 35e785d — anchors may drift)
- SemanticElement (theme.rs:29): 27 variants; FOUR chrome faces — `Chrome` (panel),
  `ChromeReverse` (modifier-only reverse), `ChromeSelected`, `ChromeMuted`. `Face`
  (theme.rs:18): all-Option fg/bg/underline_color + bold/italic/underline/strike/reverse/
  dim. `Theme { name, base_fg, base_bg, heading_level_glyph, monochrome, faces }`
  (:116); `face()` exhaustive dispatch (:125); `override_face` exists.
- Builtins (:144/:162): 13 names — default, no-color, tokyo-night, 5 phosphor × (main +
  `-flat`). tokyo_night chrome: `fg FG / bg PANEL_BG(#16161e)` vs base `#1a1b26` —
  hand-tuned, KEPT as an override. phosphor (:499): `base_bg = shade(hue,0)` — `shade`
  (:458) preserves H/S, maps level to lightness 0.08..0.92, so phosphor canvases are
  hue-tinted near-black; the ladder inherits the tint by construction (lightness-only
  steps). `-flat` = mono_faces + hue chrome (the inversion the user rejected).
  `from_base16` (:339) maps base00/05 to bg/fg + a panel guess; its chrome mapping is
  SUPERSEDED by derivation. `default()` (:228): markdown coloring is ONLY code=Cyan +
  link=Yellow (+DarkGray markers); headings/quote/emphasis are modifier/glyph-carried —
  the user's "essentially no-color for markdown" observation, driving the `terminal-ansi`
  fork. `mono_faces` (:469): modifier-only chrome (kept for cue mode).
- resolve_theme (theme_resolve.rs:84-138): depth detect → base pick (file → base16;
  name → builtin; else default) → user `styles.*` override loop (monochrome color-scrub)
  → `apply_cue_mode_glyph`. The disposition applies IN the derivation step (before
  theme/user overrides); cue-glyph remains LAST (the earlier "after cue-glyph" note was
  the map's suggestion for a patch-style axis — superseded by derivation, Codex r1 I-7). EnvSnapshot = NO_COLOR/COLORTERM/TERM.
- Render inventory (render.rs, 1,134 prod lines; full site list in the map): overlay
  UNSELECTED rows + backdrop gaps = ratatui List defaults after Clear (UNTHEMED — the
  reported bug); overlay query lines = SE::Text (unthemed in every builtin); overlay
  borders = SE::Chrome; status line = SE::ChromeReverse in ALL FOUR states (:637-686 —
  modifier-only, the menu-bar mismatch); menu bar/dropdown = Chrome/ChromeSelected/
  ChromeMuted (:985-1033); scrollbar = ChromeMuted track + Chrome thumb; the six-style
  precompute block (:724-733). Split seams: shared geometry helpers (:107-187, pub(crate),
  mouse.rs consumes), the draw path (:253-633), the overlay painters (:634-1081).
- Test breakage inventory: `all_thirteen_builtins_total` == 13 (theme.rs:721);
  theme_picker `rows.len() >= 13` (theme_picker.rs:45); `phosphor_16color_floor` iterates
  phosphor names (flat gone); `a11y_every_cued_element…` names `phosphor-amber-flat`
  (render.rs:2136); `golden_default_scrollbar_styled` pins White/Black/DarkGray chrome
  (survives — terminal-plain keeps those values); `menu_bar_row_is_filled_full_width`
  compares against compose'd Chrome values (survives value changes by construction);
  `default_status_line_still_reversed` (render.rs:1630) pins REVERSED on the status row —
  MEANING CHANGES (the status moves to explicit colors; the pin becomes
  "status row carries the Chrome face" with a terminal-plain reverse exception — see D2).
  e2e is text-only (chrome-color-safe).
- Settings (D1+A5): OTheme mirror carries `name` only; SettingsSnapshot.theme_identity
  provenance-typed; per-field extension pattern established (wrap_column precedent).
- Config: RawTheme { name, file, depth, heading_level_glyph, styles } — `chrome` lands
  beside `depth` (string, parsed at resolve, warn on unknown).

### Research (deep-research workflow, 102 agents, all findings 3-0/2-1 verified; +
`.superpowers/sdd/e4-themes-research.md`)
- Chrome value lives in being INVISIBLE (iA) — the zen rationale.
- Dark base = near-black GRAY (never pure black; Material #121212) — headroom for the
  ladder. Depth = TONAL steps (lighter = closer, "lit from the front"; Atlassian), never
  shadows; Material quantifies overlays (5..16% white, perceptually spaced, pre-blendable
  to solid colors — the TUI transfer key).
- 4-6 ROLE-NAMED surface levels is the industry consensus (Atlassian sunken/default/
  raised/overlay; M3's six with the top reserved for interaction).
- Focus/interaction = one step UP the same ladder + paired active/inactive variants —
  never a new color.
- Fg hierarchy = a small emphasis family (87/60/38% white → primary/secondary/disabled).
- Contrast invariant: the canvas must be dark enough that primary text stays readable on
  the LIGHTEST chrome face (Material's 15.8:1 rule) — encoded as a derivation test.
- Accent discipline: ONE desaturated accent, legal only for focus/selection/modified
  marks, never large fills (Sublime ships exactly one accent for two chrome roles).
- Chrome deserves its own token architecture (Sublime theme-vs-color-scheme split); tones
  DERIVED from a shared base (Sublime Adaptive: the UI "acts as if custom designed for
  your color scheme") — the fork-1 ratification.
- E4 shortlist (all MIT, official base16/palette specs): Catppuccin (explicit
  @markup.heading/strong/italic/raw/quote groups), Flexoki ("a color scheme for prose"),
  Rosé Pine, Gruvbox, Solarized, Everforest, Kanagawa. User-picked bundle below.

## D1. The derived chrome ladder (core theme.rs)

- New: `pub enum ChromeDisposition { Full, Zen }` (CORE — the shell's config parse maps
  the string to the core enum; dependency direction sound) and a PUBLIC core method
  `Theme::rederive_chrome(&mut self, disp: ChromeDisposition)` (Codex r1 I-1: ThemeFaces
  is private — a free fn returning a private struct is uncallable from the shell; the
  method mutates the six chrome faces in place). Builtin constructors call it with Full;
  resolve_theme calls it with the configured disposition BEFORE applying overrides.
  Mechanics:
  - Steps are LIGHTNESS-ONLY moves in HSL (the existing rgb_to_hsl/hsl_to_rgb; hue and
    saturation pass through — phosphor/tokyo tints carry into chrome by construction).
  - Direction by base luminance: dark base (L < 0.5) steps LIGHTER ("lit from the
    front"); light base steps DARKER. Full step sizes: panel = 1 step, overlay = 2 steps
    (exact deltas pinned by the plan against probes; targets ≈ the Material 7%/14%
    pre-blend feel and must reproduce tokyo-night-class subtlety, not ansi-blocky jumps).
  - Fg family: chrome fg = base_fg; muted fg = base_fg stepped toward base_bg
    (secondary emphasis); selected = explicit base_bg-on-base_fg (today's convention).
  - Accent: a theme-supplied seed color, desaturated toward the research's calm range;
    when the theme supplies none, a brightness-distinct tone of base_fg (same hue, offset
    lightness) — the plan pins the exact desaturation/offset math against probes. Accent is used ONLY per D2's discipline list.
  - Zen: the SAME derivation with collapsed step sizes (panel ≈ canvas + a minimal
    visible step, overlay = one subtle step, muted dimmer, accent retained but fainter).
    Zen never changes hue.
  - Exemption rule, CONCRETE (Codex r1 I-2 — no new field needed): derivation applies
    ONLY when both bases are `Color::Rgb`; `rederive_chrome` is a NO-OP otherwise (named/
    Default bases have no computable lightness — Codex m-4 made explicit). terminal-plain,
    terminal-ansi, and no-color therefore keep their EXPLICIT face tables untouched by
    construction; base16/file themes and all E4 bundles are Rgb and derive. Derivation
    runs PRE-quantize (depth downconversion applies to the derived values as to any).
- Order of application: derive (from the theme's declared bases + disposition) → the
  theme's own explicit face overrides (tokyo-night's PANEL_BG chrome; phosphor's
  hue-shade chrome KEPT ONLY where probes show the derived values differ visibly from
  today's — the implementer probes both and keeps the smaller override set that preserves
  today's look) → user `styles.*` → cue-mode glyph forcing. `no-color`/cue mode: mono
  modifier faces, untouched by derivation (monochrome themes skip it entirely).

## D2. The six-face family + render rewiring

- SemanticElement gains `ChromeOverlay` and `ChromeAccent` (config keys `chrome_overlay`
  / `chrome_accent` in element_from_key; both documented). `ChromeReverse` REMAINS as an
  element (cue-mode/mono + config back-compat) but no full-color path composes it.
- Render rewiring (the map's inventory, exhaustive):
  - Overlay interiors: the overlay RECT is filled with the ChromeOverlay face (a
    set_style after Clear — no terminal-default gaps between rows), unselected rows
    inherit it, query lines compose [ChromeOverlay] (+ the query text keeps its content
    styling), selected rows stay ChromeSelected (dropping ChromeReverse). Applies to all
    five: palette, outline, theme picker, file browser, diag quick-fix.
  - **The border/fill rule (user-reported, 2026-07-05, two defects):**
    (1) BORDERS NEVER PAINT THEIR OWN BACKGROUND — today's border composes SE::Chrome,
    whose PANEL bg (phosphor: `shade(hue,1)`, lighter than the `shade(hue,0)` canvas)
    puts a one-cell halo of lighter hue around every modal (the reported lighter-green
    strip). Border styles become FG-ONLY from the ladder (bg None); border cells inherit
    the fill beneath them. Draw order: fill the rect, then fg-only lines.
    (2) THE FILL IS DISPOSITION-HONEST: in FULL the interior is the overlay rung (an
    intentional tone above the canvas) and the border fg is a muted same-hue step — frame
    and fill read as one raised material (the research's tonal-separation default); in
    ZEN the interior collapses toward the canvas (the blend the user expected — no more
    terminal-default "slightly off" hover) and the thin border alone carries the
    separation (border-as-separator, the calm expression). ratatui mechanics verified (Codex r1 m-2): Cell::set_style PATCHES
    fields, so a bg-None border style leaves the fill's bg intact — the rule is sound;
    tests must cover BORDER and TITLE cells explicitly, not just interiors. Pins:
    border-cell bg == interior bg under phosphor-green (the halo regression, both
    dispositions); full → interior bg == ChromeOverlay bg ≠ canvas; zen → interior bg
    within the collapsed step of canvas.
  - Status line: normal state composes [Chrome] (explicit — matches the menu bar at
    last); ACTIVE states (search / minibuffer / prompt) compose [ChromeAccent] — the
    first honest "the editor is asking you something" distinction. terminal-plain's
    ChromeAccent face uses reverse+bold (modifier expression) so the plain look stays
    calm; color themes get the derived accent tone.
  - Menu bar/dropdowns: face ROLES unchanged (Chrome bar, ChromeSelected open/highlight,
    ChromeMuted dropdown-normal) — values now derived.
  - Scrollbar/wrap-guide/fold/prefix: roles unchanged.
- Accent discipline (research-mandated, spec-enforced list): ChromeAccent is legal ONLY
  for the active-prompt status state and (future) focus marks. The dirty-buffer marker is
  DEFERRED (Codex r1 I-5: status_left_text returns one string rendered as a single span —
  render.rs:196/:683; accenting the marker needs span-level status surgery, which rides
  E2's status work). Reviewers flag any other accent use.
- `default_status_line_still_reversed` (render.rs:1630) is REWRITTEN to the new contract:
  under terminal-plain the status row carries Chrome's explicit White/Black (or the
  reverse-modifier expression terminal-plain declares); under tokyo-night the status row
  bg equals the menu bar bg (THE reported-bug regression pin).

## D3. The axis: config + toggle + persistence

- Config: `[theme] chrome = "full" | "zen"` (RawTheme + ThemeConfig
  `Option<ChromeDisposition>`; parsed at resolve like depth; unknown → warn + Full;
  default Full).
- resolve_theme applies the disposition in the derivation call (D1's order). The
  ChromeDisposition reaches apply_theme so the picker/preview path re-derives correctly.
- `toggle_chrome` command ("Chrome: Full/Zen", MenuCategory::Settings, registered BEFORE
  save_settings — the journey-preservation rule). SOURCE OF TRUTH (Codex r1 Critical:
  resolve_theme destructively folds user styles into the Theme — theme_resolve.rs:122 —
  so re-deriving from `editor.theme` would smear or lose overrides): the toggle uses the
  ESTABLISHED request-flag → run-loop pattern (keymap_rebuild / settings_save precedent).
  The handler flips `editor.chrome_disposition` (new Editor field, seeded from the
  resolved config) + sets `editor.theme_rederive: bool` + status "chrome: zen"/"chrome:
  full". The RUN LOOP — which owns `cfg.theme` (the full ThemeConfig incl. user styles)
  and the EnvSnapshot — re-runs the COMPLETE resolve pipeline (base pick → derive with
  the new disposition → theme overrides → user styles → cue glyph) and applies via
  apply_theme; derive::rebuild + ensure_visible ride the existing apply path. Full
  fidelity by construction — no state reconstruction from a folded Theme.
- Persistence: the disposition joins the Save Settings inventory per-field.
  `SettingsSnapshot` gains `chrome_disposition`; `OTheme` gains
  `chrome: Option<String>` ("full"/"zen") beside `name`. The diff arm is a PLAIN string
  pair through the generic `diff_key` — NOT the provenance-typed bespoke theme arm — with
  its own per-key mask predicate
  (`mask.theme.as_ref().and_then(|t| t.chrome.as_ref()).is_some()`). NOTE this splits the
  theme mask handling: `name` keeps the provenance-collapsed sentinel (D1+A5's N-4 rule),
  `chrome` gets the ordinary per-key predicate. The UPDATE SET named in full (Codex r1
  I-3 — parse_mask today recognizes only name/file and collapses to the sentinel,
  settings.rs:186): `OTheme` gains `chrome: Option<String>`; parse_mask's private
  MaskTheme gains `chrome`; parse_mask sets the name-sentinel from name|file AND passes
  chrome through as its own key; the bespoke theme diff section gains a plain diff_key
  string arm for chrome beside the provenance arm. Interaction pinned: a --config masking
  `file` guards `name` but NOT `chrome`.
- The picker previews themes under the CURRENT disposition: `apply_theme` gains the
  derivation call (`theme.rederive_chrome(self.chrome_disposition)` before install —
  Codex r1 I-4: the preview path calls `Theme::builtin` + `apply_theme` directly,
  bypassing resolve). KNOWN pre-existing approximation, recorded not fixed: the preview
  path also bypasses user `styles.*` today (a previewed-then-committed theme lacks user
  overrides until the next full resolve) — out of scope, noted in Deferred.

## D4. Phosphor restructure

- The five `-flat` names are REMOVED from `builtin()`/`builtin_names()`; a config naming
  one warns ("theme 'phosphor-X-flat' removed; using 'phosphor-X'") and falls back to the
  base phosphor (resolve-layer mapping, not a builtin alias).
- Main phosphors: derivation over the tinted `shade(hue,0)` base reproduces hue-shaded
  chrome by construction; explicit overrides are kept ONLY where probed-different from
  today (D1's rule). Phosphor-zen = the subdued same-hue chrome `-flat` should have been.
- Test updates: `all_thirteen_builtins_total` → the new total (see D5: 19);
  `phosphor_16color_floor` drops flat iteration; the a11y cue test replaces
  `phosphor-amber-flat` with `no-color` + a zen-phosphor case; picker `>= 13` → `>= 19`.

## D5. The theme lineup

- RENAME: `default` → **`terminal-plain`** (Theme.name only; `theme::default()` the FN
  keeps its name and table). Config back-compat: `name = "default"` resolves to
  terminal-plain with a one-time warning. The picker shows `terminal-plain`.
- NEW: **`terminal-ansi`** — full markdown colorization in NAMED ANSI colors only
  (headings/emphasis/code/link/quote/markers each a named color; chrome an explicit
  named-ANSI ladder: e.g. panel Black, overlay DarkGray-stepped — exact table in the
  plan), `base_fg/bg = Color::Default` (adapts to the terminal palette). NOT monochrome.
- NEW (E4 bundle, user-picked; all via `from_base16` + official palette specs, all MIT,
  chrome DERIVED): `catppuccin-mocha`, `catppuccin-latte`, `flexoki-dark`,
  `flexoki-light`, `gruvbox-dark`, `gruvbox-light`, `rosepine-moon`, `rosepine-dawn`,
  `solarized-dark`, `solarized-light`. Each gets a markdown face mapping from its
  official spec (the research's markdown-group citations; heading/emphasis/code/link/
  quote hues chosen per the upstream theme's own markup groups where published, else the
  base16 conventions). Light themes exercise the ladder's darker-steps direction.
- **Launch default: `flexoki-dark`** — `resolve_theme`'s no-config arm (no name, no
  file) returns `Theme::builtin("flexoki-dark")` instead of `theme::default()`. Error
  fallbacks (unknown name, unreadable file, base16 parse failure) STAY on the plain
  table (minimal-safe). Depth::None still forces no-color.
- Total: terminal-plain, terminal-ansi, no-color, tokyo-night, 5 phosphor, 10 E4 = **19**.

## D6. The render.rs split (carried)

- `render.rs` keeps: the draw path (tiny-guard, canvas/text rows, scrollbar, status
  line), the shared geometry helpers (pub(crate); mouse.rs untouched), compose glue.
- Geometry helpers (palette_overlay_rect, menu_bar_layout, menu_dropdown_row_at, etc.)
  STAY in render.rs as pub(crate) — mouse.rs's `crate::render::…` imports (mouse.rs:144/
  :180/:186) remain valid unchanged (Codex r1 m-3).
- NEW `render_overlays.rs`: the five overlay painters + the menu painter + the diag
  overlay, receiving a `ChromeStyles` struct (the precompute block's six styles + the two
  new faces) built once in render.rs. Byte-identical moves where code doesn't otherwise
  change (H1 discipline); the rewiring diffs land as separate commits from the moves so
  review can verify conservation.

## Error handling

- Unknown `chrome` value: warn + Full. `-flat`/`default` names: warn + mapped fallback.
- base16 file failures: unchanged (warn + plain fallback).
- toggle_chrome under a monochrome/cue theme: no-op with status "chrome: n/a (cue mode)"
  — zen has no meaning without a ladder; pinned.
- No new IO. The toggle's re-derivation is a cold path (one keypress).

## Testing

- Ladder unit battery (core): direction by luminance (dark lightens, light darkens);
  zen steps strictly smaller than full; hue/saturation preserved through steps (the
  phosphor-tint pin); every derived face fully explicit (no None fg/bg on color themes);
  the contrast invariant (primary fg vs the LIGHTEST chrome face ≥ a pinned readable
  delta — exact metric pinned by plan probes); accent desaturation bound.
- Theme lineup: builtin_names == 19 + each new theme resolves at all three depths without
  panic (quantize sweep); "default"-alias + "-flat"-fallback warnings pinned;
  flexoki-dark launch-default pin (resolve with empty ThemeConfig → name == "flexoki-dark");
  terminal-ansi markdown faces all named-ANSI (no Rgb values — a property pin).
- Render: the reported-bug regression pins — under tokyo-night: status bg == menu bar bg;
  palette unselected-row bg == ChromeOverlay bg; overlay backdrop has NO terminal-default
  cells inside the rect; under phosphor-green: EVERY border cell's bg == the interior
  fill bg (the lighter-green halo regression), asserted in both dispositions. The rewritten status pin (D2). The prompt-active accent pin
  (open a minibuffer → status face == ChromeAccent). Menu fill + scrollbar goldens
  updated where values moved (compose-relative comparisons survive by construction).
  a11y cue-mode modifier coverage re-pinned for the new faces (ChromeOverlay/ChromeAccent
  must carry modifiers under no-color).
- Toggle/persistence: the established per-field battery (idempotent toggle status;
  chrome joins OTheme round-trip through the REAL loader; diff-law arms incl. the split
  theme mask predicates; save→reload lands the disposition).
- e2e: one journey — under tokyo-night open the palette (themed interior asserted via
  buffer cell styles at the harness level if reachable, else the render-test layer owns
  it and the journey pins text), toggle_chrome via palette → status "chrome: zen",
  Save Settings → file carries `[theme] chrome = "zen"`.
- Split conservation: moved-code commits byte-verified (H1's discipline; the whole-branch
  review charges it).
- Smoke: advisory run + verbatim quote (no new smoke checks).

## Deferred (recorded)

- E1 presets consume the axis; E2 radio marks show the active theme/disposition.
- OSC 12 hardware-cursor color; overlay mouse parity (A6 follow-up) untouched.
- A `terminal-ansi`-style bright variant, more E4 themes (Everforest/Kanagawa were
  shortlisted, cut for picker economy) — on demand.
- Upstream: none from this effort (the repar candidates ride the previous spec).
