# Chrome elevation recalibration — design

**Status:** brainstorm-approved 2026-07-06. Facts cross-checked against real source at HEAD of
`main` (post canvas-transparency merge). Branch `effort-chrome-elevation`. Gating: Codex is the
sole spec/plan gate (loop until clean); Fable reviews the whole branch only.

## Goal

Make every chrome surface — menu bar, status line, menu dropdown, overlays — read as a distinct,
elevated layer, clearly separated from the document content and from each other, on all 19 themes
(dark and light). This fixes three related, user-reported failures that share one root cause:

1. **Chrome text mistaken for document text** — the menu bar / status line render `base_fg` (the
   exact body-text color) on a background only ~18% darker than the canvas, so they blend into the
   document.
2. **Dropdown not clearly distinct from the menu bar.**
3. **`full` and `zen` chrome not visually distinct from each other.**

## Background — why the current ladder fails

E3's `derive_chrome` (`wordcartel-core/src/theme.rs:222`) uses a **split ladder**: bars sink toward
black (`chrome bg = clamp_blend(bar_pct, black)`, `CHROME_BAR_PCT_DARK = 0.18`), while overlays
already elevate toward the *headroom* pole (`ov_pole = white on dark`). The bar direction is the
bug. On a dark theme the canvas is near-black (flexoki-dark `base_bg = #100f0f`, relative luminance
~0.015): there is almost no luminance room *below* it, so an 18%-toward-black step is a tiny
absolute change (`#100f0f → #0d0c0c`, three levels) and the bar never becomes its own surface. The
existing `clamp_blend` only *shrinks* the step to preserve `base_fg`-vs-rung legibility — it never
enforces separation *from the canvas* — so nothing rescues the near-black case. `zen` scales the
already-tiny step by `ZEN_COLLAPSE = 0.35`, so `full` (~18%) and `zen` (~6%) of a near-black range
differ imperceptibly. And the bar foreground is `base_fg`, identical to body text.

The room on a dark theme is *upward* (toward white); on a light theme it is *downward*. Overlays
already exploit this. The fix generalizes that to every chrome layer and adds a separation floor.

## D1 — Unified elevation ladder

Replace the split ladder with one rule: **every derived chrome surface elevates away from the
canvas toward the pole with headroom** — lighter on dark themes (`is_dark = rel_lum(bg) < 0.5`),
darker on light themes — i.e. the overlay's existing `ov_pole` logic applied to *all* chrome
layers, not just overlays. Successive layers elevate by increasing amounts, producing a strictly
ordered, always-distinct stack:

```
canvas (base_bg)  <  bar (Chrome)  <  dropdown (ChromeMuted)  <  overlay (ChromeOverlay)
        level 0            level 1              level 2                    level 3
```

(`<` = "further from the canvas toward the headroom pole.") The face → level mapping reuses E3's
existing chrome faces unchanged in identity — this is a re-derivation, not a new face set.
`ChromeSelected` (inverted highlight), `ChromeAccent` (status prompt/active state), the scrollbar
track/thumb, and `ChromeReverse` (cue, never derived) keep their roles and compose on top.

**Hue-preserving.** The elevation shifts lightness while preserving the canvas's hue and
saturation, so each panel is a *tinted* member of the theme (a warm theme's bars stay warm, a cool
theme's stay cool) rather than a desaturated gray strip. The exact mechanism is probe-calibrated
(§Calibration) — an HSL-lightness shift on the background is the candidate, applied to the
**background only** so E3's C1-A caution (HSL math misbehaving on *derived foregrounds*, the
"yellow chrome" failure) does not apply here.

## D2 — Separation floor (the guarantee)

Each layer has a **calibrated default elevation step** (like E3's per-layer fractions), but the
step is **clamped *up*** — increased — until the layer's background clears a **minimum separation**
from the layer beneath it (canvas for the bar, bar for the dropdown, dropdown for the overlay).
Separation is a probe-calibrated target: a minimum luminance delta / modest contrast ratio.
Surfaces need only be *noticeably* distinct, not text-legible-distinct, so the floor sits well
below the 4.5:1 text threshold. This is a **new** clamp, opposite in direction to E3's existing
`clamp_blend` (which *shrinks* a step to keep the foreground legible): here we *grow* the step to
guarantee inter-surface separation. Both clamps coexist — grow to the separation floor, then, if
that would push the foreground under its legibility floor, the foreground is re-derived (D3) rather
than shrinking the surface back.

The floor is what makes distinctness hold *by construction* on the extremes — a near-black canvas
forces the bar upward far enough to be seen; a near-white canvas forces it down.

## D3 — Foreground appropriate to each panel

Once a dark theme's bar elevates toward lighter, a light `base_fg` on it can *lose* contrast — so
the chrome foreground can no longer be blindly reused from `base_fg`. Each chrome layer's
foreground **starts from `base_fg` but is contrast-derived against that layer's own (elevated)
background**: if the elevation dropped the foreground below a legibility floor, it is nudged toward
the opposite pole until it clears. The result is text that is always legible on its own panel and
distinct from document text (which sits on the un-elevated canvas). The dropdown's muted secondary
foreground (`MUTED_FG_BLEND`) and the status line's `ChromeAccent` (prompt) state keep their roles,
re-derived against their panels the same way.

## D4 — `full` vs `zen`, made distinct

`zen` collapses the elevation toward the canvas as today — **but not below the separation floor**.
So `zen` = *minimal but present* chrome (every layer still clears its floor), and `full` = a
calibrated step *clearly above* the floor (pronounced chrome). Because both are now real, floored
elevations, the gap between them is real on every theme: toggling `chrome = full|zen` produces a
visible change. **"`full` and `zen` must be visibly distinct" is an explicit calibration target**
(the full step exceeds the zen floor by a perceptible margin) **and a conformance pin**. This also
sharpens the dispositions' meaning: `zen` = "minimal present chrome," `full` = "pronounced chrome"
— cleaner than E3's "collapse toward the canvas."

## D5 — Reconciliation with E3

- Rewrites `derive_chrome`'s background derivation (split ladder → unified elevation + separation
  floor) and adds the foreground contrast-derivation. E3's split-ladder decision (I1-A) is
  **superseded** by the elevation model.
- **Unchanged in structure:** the five all-None sentinel faces + the sentinel rule (explicit
  constructor chrome still survives; only sentinels derive), the `[theme] chrome = full|zen` axis +
  `toggle_chrome` + persistence, the `[theme] canvas = opaque|transparent` axis, the Ansi16
  fixed-table policy, and the render `ChromeStyles` mapping.
- **Regenerated:** every derived-chrome hex value E3 pinned in its tests is recomputed under the new
  derivation — probe-driven, exactly as E3 calibrated its originals. The spec/plan grounding carries
  the new expected values.
- **Preserved invariants:** the status line still matches the menu bar (both `Chrome`); overlay
  interiors stay themed; the canvas effort's body-text `base_fg` fallback and the opaque/transparent
  behavior are untouched.

## Testing

A conformance test over **every RGB builtin at every color depth** asserts:
- the stack is strictly ordered and floored — `canvas → bar → dropdown → overlay`, each clearing the
  separation floor from the layer beneath;
- each chrome foreground clears its legibility floor against its own panel background;
- `full` and `zen` produce measurably distinct bar tones.

Plus the reported-bug pins (representative themes, probe-true hex): the bar foreground is
distinguishable from body text; the dropdown background is distinct from the bar; `full ≠ zen`. The
E3 reported-bug pins that still hold (`tokyo_status_matches_menu_bar` — status == bar) are retargeted
to the new derived values.

## Calibration (probe-driven, resolved in grounding/plan — as E3's fractions were)

The exact values are determined by probes during grounding, then pinned:
- the **separation floor** (luminance delta / contrast target between adjacent layers);
- the per-layer **default elevation steps** for `full`;
- the **hue-preserving mechanism** (HSL-lightness shift vs a modest hue-preserving RGB approach —
  chosen by which keeps the tint without washing out, validated across all 19 themes);
- the **foreground legibility floor** and the re-derivation nudge;
- the **`full`-vs-`zen` margin**;
- the full set of **regenerated expected hex pins** for the derived chrome faces across the pinned
  themes and both dispositions.

## Non-goals / deferred (the follow-on theme-standardization effort)

- **Theme-face completeness** (colored emphasis/strong/etc.; the completeness standard + conformance
  test).
- **Tokyo chrome → derived** — tokyo's explicit `PANEL_BG` chrome becomes a sentinel there and picks
  up this elevation derivation automatically.
- **Phosphor near-white headings/links** (the shade-ceiling fix).
- **The heading-text-color consistency decision.**
- These ride the next effort; this one is purely the chrome-ladder recalibration.
