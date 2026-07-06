# Theme rendering quality — chrome elevation + completeness standard — design

**Status:** CLEAN — Codex spec gate r1→r2→r3 (READY, empty findings), 2026-07-06. Brainstorm-approved. Facts cross-checked against real source at HEAD of
`main` (post canvas-transparency merge). Branch `effort-chrome-elevation`. Gating: Codex is the sole
spec/plan gate (loop until clean); Fable reviews the whole branch only.

## Goal

One "theme rendering quality" effort with five tightly-related parts, all raising the bar on how
themes render:

- **A. Chrome elevation recalibration** — every chrome surface reads as a distinct, elevated layer,
  separated from content and from each other, on all 19 themes.
- **B. A theme-completeness standard + conformance test** — codify what a "complete" theme must
  define, enforced so it can't silently drift again.
- **C. The heading-text-color rule** — resolve the cross-theme inconsistency via a uniform rule.
- **D. Tokyo brought to the standard** — colored roles + chrome → derived.
- **E. Phosphor shade fix** — bright shades stay hued, and phosphor's tonal hierarchy is restored.

All five stay in one effort (they touch the same surface and share the conformance test).

---

## Part A — Chrome elevation recalibration

### A-Goal
Make the menu bar, status line, menu dropdown, and overlays each read as a distinct elevated layer,
clearly separated from the document and from each other, on every theme. Fixes three user-reported
failures with one root cause: (1) chrome text mistaken for body text; (2) dropdown not distinct from
the bar; (3) `full` and `zen` chrome not distinct from each other.

### A-Background — why the current ladder fails
E3's `derive_chrome` (`wordcartel-core/src/theme.rs:222`) uses a **split ladder**: bars sink toward
black (`chrome bg = clamp_blend(bar_pct, black)`, `CHROME_BAR_PCT_DARK = 0.18`), while overlays
already elevate toward the *headroom* pole (`ov_pole = white on dark`). The bar direction is the bug.
On a dark theme the canvas is near-black (flexoki-dark `base_bg = #100f0f`, rel-lum ~0.015): there is
almost no luminance room *below* it, so an 18%-toward-black step is a tiny change (`#100f0f → #0d0c0c`)
and the bar never becomes its own surface. The existing `clamp_blend` only *shrinks* the step to keep
`base_fg`-vs-rung legible — it never enforces separation *from the canvas*. `zen` scales the tiny step
by `ZEN_COLLAPSE = 0.35`, so `full` and `zen` differ imperceptibly. And the bar foreground is `base_fg`,
identical to body text.

### A-D1 — Unified elevation ladder
Replace the split ladder with one rule: **every derived chrome surface elevates away from the canvas
toward the pole with headroom** — lighter on dark themes (`is_dark = rel_lum(bg) < 0.5`), darker on
light — i.e. the overlay's existing `ov_pole` logic applied to *all* chrome layers. Successive layers
elevate more, producing a strictly ordered, always-distinct stack:
```
canvas (base_bg)  <  bar (Chrome)  <  dropdown (ChromeMuted)  <  overlay (ChromeOverlay)
```
Face → level mapping reuses E3's existing chrome faces unchanged in identity. `ChromeSelected`
(inverted highlight), `ChromeAccent` (status prompt — reads `chrome.bg`, which now elevates),
scrollbar track/thumb, and `ChromeReverse` (cue, never derived) keep their roles on top.
**Hue-preserving:** the elevation shifts lightness while preserving the canvas hue+saturation (a tinted
panel, not a gray strip); mechanism probe-calibrated (§Calibration), applied to the **background only**
so E3's C1-A HSL caution (on derived *foregrounds*) does not apply.

### A-D2 — Separation floor (the guarantee)
Each layer has a calibrated default step, **clamped *up*** until its background clears a **minimum
separation** (probe-calibrated luminance delta / modest contrast, well below the 4.5:1 text threshold)
from the layer beneath (canvas for bar, bar for dropdown, dropdown for overlay). This is a **new**
clamp — opposite in direction to E3's existing legibility clamp (which *shrinks*). They coexist: grow
to the separation floor; if that pushes the foreground under its legibility floor, re-derive the
foreground (A-D3) rather than shrink the surface back. The floor is what makes distinctness hold *by
construction* on near-black / near-white canvases.

### A-D3 — Foreground appropriate to each panel
Each chrome layer's foreground **starts from `base_fg` but is contrast-derived against that layer's own
(elevated) background**: if elevation dropped it under a legibility floor, it's nudged toward the
opposite pole until it clears. Result: text always legible on its own panel and distinct from document
text (which sits on the un-elevated canvas). The dropdown's muted secondary fg (`MUTED_FG_BLEND`) and
the status `ChromeAccent` state keep their roles, re-derived against their panels.

### A-D4 — `full` vs `zen`, made distinct
`zen` collapses the elevation toward the canvas **but not below the separation floor** → *minimal but
present* chrome. `full` = a calibrated step *clearly above* the floor → *pronounced* chrome. Because
both are real floored elevations, the gap is real on every theme. **"`full` and `zen` visibly
distinct" is an explicit calibration target and a conformance pin.**

### A-D5 — Reconciliation with E3
Rewrites `derive_chrome`'s background derivation (split → unified elevation + separation floor) and adds
the foreground contrast-derivation. E3's split-ladder decision (I1-A) is superseded. **Unchanged in
structure:** the five all-None sentinel faces + the sentinel rule, the `[theme] chrome = full|zen` axis
+ `toggle_chrome` + persistence, the `[theme] canvas = opaque|transparent` axis, the Ansi16 fixed-table
policy, the render `ChromeStyles` mapping. **Regenerated / rewritten:** every derived-chrome hex value E3 pinned is recomputed
under the new derivation (probe-driven) — AND the E3 tests that pin the *old direction* as a semantic
invariant are **rewritten** as new elevation/floor invariants, not merely re-hexed (Codex spec r1):
`derive_split_ladder_directions` (bar DARKER than canvas → now bar ELEVATED from canvas, direction
polarity-dependent; theme.rs:1363), the zen `bar <= canvas` assertion (→ zen floored-but-distinct;
theme.rs:1414), `derive_saturation_split` (sunken-vs-raised rungs → the unified-elevation rungs;
theme.rs:1433), and the low-contrast clamp flooring to canvas (→ the separation-floor behavior;
theme.rs:1495). **Preserved invariants:** status line matches the
menu bar; overlay interiors themed; the canvas effort's body-text `base_fg` fallback + opaque/transparent
behavior untouched.

---

## Part C — The heading-text-color rule (resolved first; it frames B, D, E)

**Decision: colored per-level heading text, everywhere, via a uniform `SE::Text`-empty rule.**

Today heading text is inconsistent: tokyo + base16 color it per level (magenta H1, blue H2, …) because
their `text` face is empty and the heading role shows through; phosphor renders heading text in `base_fg`
because its `text = shade(hue,3)` overrides the heading role (the compose stack is `[Text, role,
style_element(Plain)=Text]`, and a non-empty trailing `Text` clobbers the role fg).

**The rule:** `SE::Text` is **empty (`Face::default()`) in every theme.** Body text is then supplied
uniformly by the render `base_fg` fallback (shipped in the canvas effort: a span with no composed fg
falls back to `base_fg`). Every colored role (heading, emphasis, …) carries its own fg, which the empty
`Text` no longer clobbers. This:
- makes heading text colored per-level in **every** theme (the role fg survives),
- fixes phosphor's inconsistency: emptying its `text` (currently `shade(3)` == `base_fg`) leaves body
  text unchanged (fallback → `base_fg` == `shade(3)`) but frees headings to render their brighter
  shades (`s(5)/s(4)/s(3)`) — restoring phosphor's real tonal hierarchy (dim comments, mid body, bright
  headings), which today collapses to flat `base_fg`,
- removes phosphor's `Text`-face special case entirely.

**Monochrome preserved by construction:** "colored per-level" means a distinct *foreground* per level —
a distinct **hue** for polychrome themes (base16) OR a distinct **shade of the one hue** for monochrome
themes (phosphor). The rule never requires multiple hues, so phosphor stays all-green/all-amber; it just
gains a proper bright-to-dim shade hierarchy.

**Scope — LIVE PREVIEW only (Codex spec r1):** the colored-per-level heading text is the *live-preview*
rendering. **Source mode** (the raw-markdown view) composes only `SE::Text` (+ optional `FocusDim`) —
never `role_element`/`style_element` (render.rs:552/628) — so with `SE::Text` empty, source-mode text
falls through the `base_fg` fallback to a **uniform `base_fg`** with no semantic color. That is the
correct, unchanged behavior for a raw-source view (a heading line shows its raw `# ` markup in plain
body color). Part C's colored-per-level claim applies to live preview; source mode stays uniform.

---

## Part B — The theme-completeness standard + conformance test

**The standard (written contract in the spec, enforced by a test):** a *complete* theme distinguishes
every semantic role via a **distinct foreground** — satisfiable by a distinct hue (polychrome) OR a
distinct shade of the theme's hue (monochrome). Strictness level **(b) "styled where base16 styles"** —
but the requirement is **per-face by TYPE**, not a blanket `fg` (Codex spec r1: base16 styles some roles
via `underline_color` or `bg`, not `fg`). The contract, derived from the `from_base16` template, assigns
each face a requirement:
- **fg-required** (text-color roles): `emphasis`, `strong`, `strong_emphasis`, `code`, `strikethrough`,
  `link`, `heading[0..6]`, `block_quote`, `code_block`, `list_marker`, `thematic_break`, `front_matter`,
  `comment`, `focus_dim`, `fold_marker`, `wrap_guide` — each MUST carry an explicit non-default `fg` (a
  distinct hue for polychrome themes, a distinct shade for monochrome).
- **underline_color-required** (diagnostics): `diag_spelling`, `diag_grammar` — MUST carry
  `underline_color` (base16/tokyo style these via underline, not fg; so tokyo's `diag_grammar` with no
  `fg` is CORRECT under the standard).
- **highlight-required**: `selection`, `marked_block`, `search_match` — MUST carry a `bg` **OR** a
  highlight modifier (`reverse`). Polychrome themes (base16, tokyo) use a `bg` color; **phosphor uses
  `reverse` (+ underline), which is valid and monochrome-consistent** — the same "distinguish by color
  OR modifier" principle as the a11y cue rule, so phosphor's `bg: None` selection PASSES the standard
  (Codex spec r2).
- **modifier-required**: `search_current` (reverse) — MUST carry ≥1 modifier.
- **intentionally empty**: `SE::Text` (Part C — body text via the `base_fg` fallback).
- **derived** (exempt from the per-face list; supplied by the elevation ladder): the six chrome faces.
The terminal-* / no-color themes are exempt entirely (non-Rgb `base_bg`); the a11y cue rule still governs
cue mode. The exact list above is the load-bearing contract; grounding confirms it against the real
`from_base16` template and every RGB builtin (each must already pass or be a documented Part D/E fix).

**Conformance test** (extends the existing `builtin_names_final_nineteen` loop, `theme.rs:1053`): over
the **16 RGB builtins** (tokyo + 5 phosphor + 10 base16; the 3 terminal-*/no-color themes are exempt),
assert each face satisfies **its** requirement type from the contract above (fg / underline_color / bg /
modifier); assert `SE::Text` is empty; assert the a11y cue contract still holds. This is the "stronger
spec" guardrail — a new theme that leaves a required role unstyled fails the build.

---

## Part D — Tokyo brought to the standard

Fix the gaps Codex's field-by-field comparison found (using tokyo's own palette constants, all confirmed
present at `theme.rs:444-456`), keeping tokyo bespoke (Codex advised against a base16 conversion):

| Face | Current | Standardized |
|---|---|---|
| `emphasis` | italic only | `fg: MAGENTA`, italic |
| `strong` | bold only | `fg: YELLOW`, bold |
| `strong_emphasis` | bold+italic | `fg: ORANGE`, bold, italic |
| `strikethrough` | strike only | `fg: COMMENT`, strike |
| `search_match` | `bg: SEL_BG` only | `bg: YELLOW`, `fg: BG` (contrast) |
| `front_matter` | `fg: DARK3` | `fg: ORANGE`, italic |
| `diag_grammar` | `underline_color: YELLOW` | `underline_color: BLUE` |
| `wrap_guide` | `fg: DARK3` | `fg: SEL_BG` |
| `selection` | `bg: SEL_BG` (#283457) | `bg: SEL_BG` — **align the `SEL_BG` constant → #292e42** (Folke `bg_highlight`; source-verified — our #283457 matches no documented Folke color; the `SEL_BG` constant currently feeds `selection.bg` (theme.rs:486) + `search_match.bg` (theme.rs:489); AFTER
Part D it feeds `selection.bg` + `wrap_guide.fg` — `search_match` moves to `bg: YELLOW` — so aligning the
constant hits all its live consumers, Codex spec r2) |
| `chrome` / `chrome_selected` / `chrome_muted` | explicit `PANEL_BG` | **`Face::default()` sentinels** — derived by Part A's elevation ladder (user decision: PANEL_BG was a direction, not a standard) |
| `text` | already `Face::default()` | unchanged (Part C rule) |

Tokyo's headings already color per-level and its palette is otherwise complete; after this it passes
Part B's conformance test and picks up Part A's elevated chrome automatically.

---

## Part E — Phosphor shade fix

`phosphor()` (`theme.rs:844`) builds all faces from `shade(hue, level)`, where `shade` maps level 0–5 to
HSL lightness **0.08–0.92** (`theme.rs:799`). Level 5 (lightness 0.92) washes any hue to near-white, so
H1/H2 (`s(5)`) and links (`shade(hue,5)`) render near-white instead of phosphor-colored.

**Fix: cap the ramp's lightness ceiling** (0.92 → a probe-calibrated ~0.78) so the brightest shade stays
clearly hued. This fixes headings and links together in one change, uniformly across all five phosphor
variants (they share the constructor). The cap also incidentally shifts the other `shade(hue,5)` uses —
`selection.fg` (theme.rs:855) and `diag_spelling.underline_color` (theme.rs:861/865) — which the grounding
pins as intended, not just headings/links (Codex spec r1). Combined with Part C (emptying phosphor's
`text`), phosphor's headings render in these corrected bright shades — the authentic dim→bright monochrome
CRT hierarchy.

---

## Testing (unified)

- **Chrome conformance** (Part A): every RGB builtin, every color depth — stack strictly ordered and
  floored (`canvas → bar → dropdown → overlay` each clear the separation floor); each chrome foreground
  clears its legibility floor against its own panel; `full ≠ zen` measurably. Reported-bug pins: bar fg
  distinguishable from body text; dropdown bg distinct from bar bg; `full ≠ zen`.
- **Completeness conformance** (Part B): every RGB builtin — every must-be-colored face has an explicit
  fg; `SE::Text` empty; a11y cue contract holds.
- **Heading pins** (Part C): a base16 theme AND phosphor render heading text in the *role* fg (colored /
  shaded), not `base_fg`; phosphor body text stays `base_fg`.
- **Tokyo pins** (Part D): the eight face changes render their new values; tokyo passes conformance;
  tokyo's chrome is now derived (elevated), status == bar still holds.
- **Phosphor pins** (Part E): `s(5)` is hued (not near-white) after the cap; heading/link shades pinned.
- Regenerated E3 chrome hex pins throughout.

## Calibration (probe-driven, resolved in grounding/plan — as E3's fractions were)

Determined by probes during grounding, then pinned: the **separation floor**; the per-layer **default
elevation steps** (`full`); the **hue-preserving mechanism** (HSL-lightness vs modest hue-preserving RGB,
chosen by which keeps tint without washing out, validated across all 19 themes); the **foreground
legibility floor** + re-derivation nudge; the **`full`-vs-`zen` margin**; the **phosphor lightness
ceiling** (~0.78); the **must-be-colored face list** (from the `from_base16` template); and the full set
of **regenerated expected hex pins** for the derived chrome faces + the tokyo/phosphor face values.

## Non-goals / deferred

- No new themes; no new config axes (chrome/canvas axes unchanged).
- E1+E2 density presets, R1 responsiveness, the repar re-plumb — separate efforts per the working order.
