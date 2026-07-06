# Theme Rendering Quality — Chrome Elevation + Completeness Standard Implementation Plan

> Plan status: DRAFT — pending Codex plan gate (loop until GO) + Fable whole-branch review.
> Spec (CLEAN): `docs/superpowers/specs/2026-07-06-wordcartel-chrome-elevation-design.md`
> (Codex spec gate r1→r3 READY). Grounding (verbatim sites + probe-calibration + regenerated
> pins): `.superpowers/sdd/theme-quality-grounding.md`.

> **For agentic workers:** REQUIRED SUB-SKILL — use `superpowers:subagent-driven-development`
> (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking. Every code step is TDD: write the failing test → run it
> RED → implement → run it GREEN → gates → commit.

**Goal:** raise theme rendering quality across five tightly-coupled parts on one branch
(`effort-chrome-elevation`): **(A)** replace E3's split chrome ladder with one **HSL-lightness
elevation** ladder so every chrome surface is a distinct, tinted, elevated layer on all 19 themes,
with `full` and `zen` provably distinct; **(B)** codify + enforce a **theme-completeness standard**
(a conformance test over the 16 RGB builtins); **(C)** a uniform **`SE::Text`-empty** rule so
heading text colours per level in every theme; **(D)** bring **tokyo-night** to the standard
(coloured roles + derived chrome); **(E)** cap the **phosphor shade** ramp so bright shades stay
hued.

**Architecture:** five tasks along the spec's five parts. T1 is the core — it rewrites
`derive_chrome` (new mechanism + constants) and, in the SAME commit, regenerates every derived-chrome
pin the rewrite breaks (the base16 sentinel-derived themes) and rewrites the four E3 semantic tests
whose *direction* invariants are superseded. T2 empties phosphor's `text` face (Part C). T3 brings
tokyo to the standard (8 face changes + `SEL_BG` alignment + chrome → sentinels → derived). T4 caps
the phosphor `shade` ceiling (Part E). T5 adds the completeness conformance test (Part B). Each task
gates green independently.

**Tech Stack:** Rust; `wordcartel-core` (theme derivation, `#![forbid(unsafe_code)]`) +
`wordcartel` shell (render/compose tests). No new dependencies.

---

## Global Constraints

- **Grounding is probe/source truth.** `.superpowers/sdd/theme-quality-grounding.md` §II.5 (chrome
  hex pins), §II.5a (synthetic low-contrast pins), §II.6 (phosphor shade table), and Part I
  (verbatim code sites) are the load-bearing deliverable. **Transcribe every hex byte-for-byte; a
  failing pin means TRANSCRIPTION ERROR first, then BLOCKED with evidence — never adjust a literal or
  hand-tune a constant to make a pin pass.** Pins carried into a test are marked `[verify]`: the
  implementer confirms them against the SHIPPED `derive_chrome` at implementation time (the values
  reproduce because the u8 quantization boundary is what §II.5 fixes — see the granularity note
  below).

- **The calibrated constants (grounding §II.2, verbatim — these ARE the constants):**
  `SEP_FLOOR_CR = 1.12` (zen adjacent-layer CR target); `FULL_STEP_CR = 1.30` (full adjacent-layer CR
  target) — **full and zen are the SAME elevation algorithm with different CR targets, which
  guarantees `full ≠ zen` by construction**; `FG_FLOOR = 4.5` (each chrome fg clears 4.5 vs its own
  panel, capped by the pole-vs-panel max CR); `CHROME_PANEL_S_CAP = 0.35`, applied **`if !is_dark`
  ONLY** (light-polarity scoping — a uniform cap would wash out phosphor/solarized-dark; grounding
  §II.7 is a load-bearing finding); phosphor `shade` lightness ceiling **0.92 → 0.78**. **KEEP**
  `MUTED_FG_BLEND = 0.35`, `ACCENT_DESAT = 0.50`, and the zen accent extra `0.40` (named
  `ZEN_ACCENT_EXTRA`). **DELETE** the six `CHROME_*_PCT_*` fractions + `ZEN_COLLAPSE`.

- **Two implementation-granularity constants (reconstruction — see the GROUNDING GAP note):**
  `LAYER_L_STEP` and `FG_NUDGE_STEP`. The probe source that generated §II.5 (`scratchpad/probework/probe.rs`)
  is NOT preserved in-repo, so the exact lightness/pct search granularity of the elevation mechanism
  is not pinned as a named constant. It does not need to be: the §II.5 pins are u8-quantized RGB, and
  any step **finer than one u8 of lightness (≈ 1/255 ≈ 0.0039)** visits every u8 panel without
  skipping, so the *first* u8-quantized panel clearing the CR target is deterministic and identical
  for every such step. Use `LAYER_L_STEP = 0.001` and `FG_NUDGE_STEP = 0.002`. **If the shipped code
  does not reproduce §II.5 byte-exact, that is a BLOCKED escalation** (re-derive the mechanism / re-run
  a probe against the real helpers) — do NOT edit the pins.

- **Gates after EVERY commit (all three, on the merged working tree):**
  `cargo test -p wordcartel-core -p wordcartel` green;
  `cargo clippy --workspace --all-targets` clean (`[workspace.lints.clippy] all = "deny"` is LIVE — a
  new warning fails the run; any deliberate exception is an item-local `#[allow(clippy::…)]` with a
  one-line rationale, never a blanket allow);
  `cargo build` warning-free for the touched crate(s).
- **Do NOT run `cargo fmt`** (hand-formatted dense house style, no `rustfmt.toml`). Match neighbours by
  hand; do not reflow untouched code.
- **House style:** `—` (em-dash) in prose comments, never `--`; no emoji or emoji-like unicode in code
  (multibyte test corpora `é`/`中`/`🙂` are the only exception, not relevant here); 4-space indent;
  private struct fields + accessors; typed errors to the status line.
- **No catch-all `_` arms** on `SemanticElement`, `ChromeDisposition`, `CanvasMode`, or the render
  `SE`/mode matches — the compiler must force every new-variant site (these are exhaustive on purpose).
- Line anchors are HEAD references from the grounding; after drift, locate by the quoted code.
- **Exclude `Cargo.lock` drift** from every commit.
- **Trailers on every commit, verbatim** (use `git commit -F -` with a quoted `'EOF'` heredoc — a `!`
  in a double-quoted `-m` breaks zsh):
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

### GROUNDING GAP / sequencing notes (surfaced for the reviewer — do not silently resolve)
1. **Probe source not preserved.** §II.5 pins are byte-exact to a probe (`scratchpad/probework/probe.rs`)
   that is gone; the elevation mechanism here is reconstructed from the §II.1/§II.2 prose. The
   reconstruction reproduces the spot-checked pins (flexoki-dark FULL Chrome `#2a2828`, ZEN Chrome
   `#1e1c1c`, Muted `#3e3a3a` all verified by hand against the CR targets), but the FULL set is
   `[verify]`; a mismatch is BLOCKED, never a pin edit.
2. **Phosphor derived-chrome FG pins are post-T4.** §II.5's phosphor-green pins (`#004000/#00ff00`
   etc.) were probed with the 0.78 ceiling ALREADY applied. Phosphor `base_bg = shade(hue,0)` is
   ceiling-invariant (L=0.08 both), so the derived-chrome **bg** channels (`#004000/#005400/#006800`)
   are stable across T4 — but `base_fg = shade(hue,3)` shifts `#2bff2b → #00ff00` and `link = shade(hue,5)`
   shifts under T4, so the derived-chrome **fg/accent** channels change at T4. Therefore **T1 pins the
   phosphor derived-chrome BG channels + invariants only; T4 adds the exact phosphor derived-chrome FG
   pins (§II.5).** Base16 themes (flexoki/mocha/gruvbox/solarized) have stable bases, so their §II.5
   pins are fully applied in T1.
3. **Tokyo's overlay pin is an intermediate value at T1, final at T3 — in TWO files.** At T1, tokyo's
   `chrome`/`chrome_muted`/`chrome_selected` are still EXPLICIT (Part D lands in T3), and
   `chrome_muted.bg == None`, so at T1 only `chrome_overlay` + `chrome_accent` derive, and the overlay
   elevates from the explicit `chrome.bg = PANEL_BG #16161e` (the muted-bg fallback). The T1 derivation
   rewrite CHANGES that overlay away from the old `#2f303a`, so **every test pinning tokyo's derived
   overlay breaks at T1 AND again at T3** — namely `derive_fills_only_unset_faces` (theme.rs:1325) and
   `rederive_respects_picker_committed_theme` (app.rs:5150/:5161). T1 pins the intermediate `[verify]`
   from the shipped code; T3 REPLACES it with §II.5's FULL `#4e5071` / ZEN `#33354a`. Tokyo's accent
   (`#16161e/#8fa3ce`) is unchanged at T1 (its bg = explicit chrome.bg, its seed = tokyo BLUE link,
   both untouched until T3). (Codex plan-gate r1 assigned the app.rs tokyo pins to T3 only; they in
   fact break at T1 too, so T1 owns the intermediate update and T3 owns the final.)
4. **The 256-color rung test needs a fresh probe.** `derived_rungs_distinct_at_256` (theme.rs:1082)
   pins the OLD split-ladder Indexed256 behaviour (`chrome`/`muted` COLLAPSE onto the canvas index;
   only `overlay` distinct). The new elevation model INVERTS this — every layer is separated from the
   canvas by a guaranteed CR floor, so at 256 colours the rungs quantize to DISTINCT (elevated)
   indices, not collapsed. This is a 5th semantic test-rewrite (beyond the four in grounding §I.9).
   §II.5's pins are truecolor; **the new Indexed256 indices are NOT in the grounding and must be
   regenerated by the T1 implementer via a probe** (quantize the shipped truecolor rungs at
   `Depth::Indexed256`) and pinned `[verify]`. If the probe is uncertain or unavailable, that is a
   BLOCKED escalation — never invent an index.

---

### Task 1: rewrite `derive_chrome` (the elevation ladder) + regenerate all chrome pins it breaks

The core. New mechanism (HSL-lightness elevation, separation-floor CR targets, light-scoped S-cap,
fg re-derivation) + new constants; DELETE the old fractions. Because this changes every
sentinel-derived chrome value, the SAME commit regenerates the base16 derived-chrome pins and rewrites
the FIVE E3 semantic tests whose direction invariants are superseded (the four in grounding §I.9 plus
the Indexed256 rung test — GAP note 4), AND updates the cross-crate pins in `wordcartel/src/app.rs`
that assert old derived-chrome hexes through the real rederive path.

**Files:**
- Modify: `wordcartel-core/src/theme.rs`, `wordcartel/src/app.rs` (tests only), `wordcartel/src/theme_resolve.rs` (tests only — `chrome_key_parses_and_derives`; Codex plan r2), `wordcartel/src/theme_picker.rs` (tests only — `preview_derives_zen_chrome_bg_on_flexoki_dark`; Codex plan r3).

**Interfaces:**
- Rewrites the body of `pub fn Theme::derive_chrome(&mut self, disp: ChromeDisposition)`
  (theme.rs:221) — same signature, same sentinel/idempotence/early-return contract; only the bg
  derivation (split `clamp_blend` → unified `next_layer`) and the fg derivation (`base_fg` verbatim →
  re-derived via `derive_fg`) change.
- Replaces the constant block (theme.rs:334-345): removes `CHROME_BAR_PCT_DARK`,
  `CHROME_BAR_PCT_LIGHT`, `CHROME_OVERLAY_PCT_DARK`, `CHROME_OVERLAY_PCT_LIGHT`, `CHROME_DEEP_PCT_DARK`,
  `CHROME_DEEP_PCT_LIGHT`, `ZEN_COLLAPSE`; adds `SEP_FLOOR_CR`, `FULL_STEP_CR`, `FG_FLOOR`,
  `CHROME_PANEL_S_CAP`, `LAYER_L_STEP`, `FG_NUDGE_STEP`, `ZEN_ACCENT_EXTRA`; keeps `MUTED_FG_BLEND`,
  `ACCENT_DESAT`.
- Reuses verbatim: `blend`, `rel_lum`, `contrast_ratio`, `equal_lum_gray`, `rgb_to_hsl`, `hsl_to_rgb`
  (all in-module — no visibility change).
- Consumers unchanged in behaviour: the render `ChromeStyles` mapping, `face()`/`face_mut()`, the
  sentinel rule, the Ansi16 fixed-table policy (`resolve_theme`), the `rederive_theme_if_requested`
  path — but the DERIVED-VALUE pins those tests assert are regenerated here.
- **Cross-crate pin sites this rewrite breaks:** `app.rs:5093/:5104` (flexoki-dark FULL/ZEN Chrome bg,
  via `rederive_theme_if_requested`) and `app.rs:5150/:5161` (tokyo FULL/ZEN ChromeOverlay bg, via
  `rederive_respects_picker_committed_theme`). flexoki is stable → §II.5 final; tokyo is the T1
  intermediate (GAP note 3).

- [ ] **Step 1 (RED): the new pin battery + rewritten invariant tests.** Add/replace tests in the
  `#[cfg(test)] mod tests` of `theme.rs`. Transcribe §II.5 hex FIRST (byte-for-byte, `[verify]`).

  **1a. Regenerate the base16 sentinel-derived chrome pins** — a new test `derive_chrome_base16_pins`
  using the existing constructors (`flexoki_dark()`, `flexoki_light()`, `catppuccin_mocha()`,
  `gruvbox_dark()`, `solarized_dark()`, `solarized_light()`) and the `assert_face_bg_fg` helper
  (theme.rs:1284). FULL and ZEN. Every value from §II.5 (`[verify]`):
  ```rust
      #[test]
      fn derive_chrome_base16_pins() {
          // §II.5 — the base16 sentinel-derived chrome, FULL + ZEN, byte-exact [verify].
          // Bases are stable across the branch, so these pins are final at T1.
          let cases: &[(fn() -> Theme, ChromeDisposition,
                        (u8,u8,u8),(u8,u8,u8),  // Chrome        bg,fg
                        (u8,u8,u8),(u8,u8,u8),  // ChromeMuted   bg,fg
                        (u8,u8,u8),(u8,u8,u8),  // ChromeOverlay bg,fg
                        (u8,u8,u8),(u8,u8,u8),  // ChromeSelected bg,fg
                        (u8,u8,u8),(u8,u8,u8),  // ChromeAccent  bg,fg
                        &str)] = &[
              (flexoki_dark, ChromeDisposition::Full,
               (0x2a,0x28,0x28),(0xce,0xcd,0xc3), (0x3e,0x3a,0x3a),(0xa5,0xa5,0x9f),
               (0x50,0x4b,0x4b),(0xce,0xcd,0xc3), (0xce,0xcd,0xc3),(0x10,0x0f,0x0f),
               (0x2a,0x28,0x28),(0x62,0x83,0xa0), "flexoki-dark FULL"),
              (flexoki_dark, ChromeDisposition::Zen,
               (0x1e,0x1c,0x1c),(0xce,0xcd,0xc3), (0x28,0x26,0x26),(0x8e,0x8d,0x86),
               (0x32,0x2f,0x2f),(0xce,0xcd,0xc3), (0xce,0xcd,0xc3),(0x10,0x0f,0x0f),
               (0x1e,0x1c,0x1c),(0x6e,0x82,0x94), "flexoki-dark ZEN"),
              // …flexoki-light, mocha, gruvbox-dark, solarized-dark, solarized-light
              //   (both dispositions) — every row from grounding §II.5, [verify].
          ];
          for &(ctor, disp,
                 c_bg, c_fg, m_bg, m_fg, o_bg, o_fg, s_bg, s_fg, a_bg, a_fg, label) in cases {
              let mut t = ctor();
              t.derive_chrome(disp);
              assert_face_bg_fg(t.face(SemanticElement::Chrome),         c_bg, c_fg, label);
              assert_face_bg_fg(t.face(SemanticElement::ChromeMuted),    m_bg, m_fg, label);
              assert_face_bg_fg(t.face(SemanticElement::ChromeOverlay),  o_bg, o_fg, label);
              assert_face_bg_fg(t.face(SemanticElement::ChromeSelected), s_bg, s_fg, label);
              assert_face_bg_fg(t.face(SemanticElement::ChromeAccent),   a_bg, a_fg, label);
              assert_eq!(t.face(SemanticElement::ChromeMuted).dim, Some(true), "{label} muted dim");
              assert_eq!(t.face(SemanticElement::ChromeAccent).bold, Some(true), "{label} accent bold");
          }
      }
  ```
  Transcribe ALL rows (flexoki-light, mocha, gruvbox-dark, solarized-dark, solarized-light — FULL +
  ZEN) from §II.5 into `cases`. (gruvbox-light and rosepine-dawn are covered by the completeness
  sweep in T5; their §II.5 pins may also be added here for coverage but are not required for T1.)

  **1b. Rewrite the four E3 semantic tests** (§I.9 — the *direction* invariants are superseded):
  - `derive_split_ladder_directions` (theme.rs:1362) → rename intent to **elevation-direction**:
    ```rust
      #[test]
      fn derive_elevation_ladder_directions() {
          // Unified elevation: every derived chrome bg elevates from the canvas toward the
          // headroom pole (LIGHTER on dark themes, DARKER on light), strictly ordered
          // canvas < bar < dropdown < overlay by luminance-toward-pole. §II.5 pins [verify].
          let mut td = flexoki_dark();
          td.derive_chrome(ChromeDisposition::Full);
          let lum = |c: Color| { if let Color::Rgb{r,g,b} = c { rel_lum(r,g,b) } else { 0.0 } };
          let canvas = lum(Color::Rgb{r:0x10,g:0x0f,b:0x0f});
          let bar  = lum(td.face(SemanticElement::Chrome).bg.unwrap());
          let drop = lum(td.face(SemanticElement::ChromeMuted).bg.unwrap());
          let ov   = lum(td.face(SemanticElement::ChromeOverlay).bg.unwrap());
          assert!(canvas < bar && bar < drop && drop < ov,
              "dark theme: canvas < bar < dropdown < overlay by luminance; \
               canvas={canvas} bar={bar} drop={drop} ov={ov}");
          // exact §II.5 pins (redundant with 1a but keeps this test self-contained) [verify]
          assert_face_bg_fg(td.face(SemanticElement::Chrome),
              (0x2a,0x28,0x28), (0xce,0xcd,0xc3), "fd chrome");

          // light polarity: elevation goes DARKER (toward black), still strictly ordered.
          let mut tl = flexoki_light();
          tl.derive_chrome(ChromeDisposition::Full);
          let canvas_l = lum(Color::Rgb{r:0xff,g:0xfc,b:0xf0});
          let bar_l  = lum(tl.face(SemanticElement::Chrome).bg.unwrap());
          let drop_l = lum(tl.face(SemanticElement::ChromeMuted).bg.unwrap());
          let ov_l   = lum(tl.face(SemanticElement::ChromeOverlay).bg.unwrap());
          assert!(canvas_l > bar_l && bar_l > drop_l && drop_l > ov_l,
              "light theme: canvas > bar > dropdown > overlay by luminance");
          assert_face_bg_fg(tl.face(SemanticElement::Chrome),
              (0xe5,0xdf,0xc8), (0x10,0x0f,0x0f), "fl chrome");  // §II.5 (S-capped) [verify]
      }
    ```
  - The zen `bar <= canvas` block in `derive_zen_collapses_toward_poles` (theme.rs:1414-1423) →
    **zen floored-but-distinct, on the pole side** (dark: `canvas_lum < zen_bar_lum < full_bar_lum`):
    ```rust
          // zen bar is strictly between canvas and the full bar, on the pole side (dark → white).
          let full_bar = lum(td_full.face(SemanticElement::Chrome).bg.unwrap());
          let zen_bar  = lum(td_zen.face(SemanticElement::Chrome).bg.unwrap());
          let canvas   = lum(Color::Rgb{r:0x10,g:0x0f,b:0x0f});
          assert!(canvas < zen_bar && zen_bar < full_bar,
              "dark: canvas < zen bar < full bar; canvas={canvas} zen={zen_bar} full={full_bar}");
    ```
    Re-hex its §II.5 flexoki-dark ZEN pins (`Chrome #1e1c1c`, `Muted #28…`/`#8e8d86`,
    `Overlay #322f2f`, `Accent #1e1c1c/#6e8294`).
  - `derive_saturation_split` (theme.rs:1433) → **all-rungs-preserve-canvas-S** (no sunken/raised
    split; §II.4: every rung's HSL-S ≈ canvas S on dark/uncapped themes):
    ```rust
      #[test]
      fn derive_rungs_preserve_canvas_saturation() {
          let mut t = catppuccin_mocha();          // dark, uncapped, canvas S ≈ 0.21
          t.derive_chrome(ChromeDisposition::Full);
          let (_, canvas_s, _) = rgb_to_hsl(0x1e, 0x1e, 0x2e);
          for el in [SemanticElement::Chrome, SemanticElement::ChromeMuted, SemanticElement::ChromeOverlay] {
              if let Color::Rgb { r, g, b } = t.face(el).bg.unwrap() {
                  let (_, s, _) = rgb_to_hsl(r, g, b);
                  assert!((s - canvas_s).abs() < 0.02,
                      "{el:?} preserves canvas S: rung_s={s:.4} canvas_s={canvas_s:.4}");
              } else { panic!("non-Rgb rung"); }
          }
      }
    ```
  - `derive_contrast_clamp_floors_at_zero` (theme.rs:1494) → **separation-floor** (no shrink-to-canvas;
    the floor GROWS the rungs and `derive_fg` re-derives the fg). Keep the synthetic theme construction
    verbatim (bg `#f8f8f8`, fg/link `#e0e0e0`); replace the assertions with §II.5a pins + the
    grow/legible invariants:
    ```rust
      #[test]
      fn derive_separation_floor_grows_low_contrast_theme() {
          // (…synthetic theme construction unchanged from theme.rs:1500-1523…)
          t.derive_chrome(ChromeDisposition::Full);
          // §II.5a FULL pins (synthetic bg #f8f8f8, fg/link #e0e0e0), [verify]:
          assert_face_bg_fg(t.face(SemanticElement::Chrome),
              (0xdb,0xdb,0xdb), (0x60,0x60,0x60), "synthetic chrome");
          assert_face_bg_fg(t.face(SemanticElement::ChromeMuted),
              (0xc1,0xc1,0xc1), (0x4f,0x4f,0x4f), "synthetic muted");
          assert_face_bg_fg(t.face(SemanticElement::ChromeOverlay),
              (0xa9,0xa9,0xa9), (0x3c,0x3c,0x3c), "synthetic overlay");
          // rungs are DISTINCT from canvas (elevated toward black), and every fg clears 4.5.
          let canvas = Color::Rgb{r:0xf8,g:0xf8,b:0xf8};
          for el in [SemanticElement::Chrome, SemanticElement::ChromeMuted, SemanticElement::ChromeOverlay] {
              let f = t.face(el);
              assert_ne!(f.bg.unwrap(), canvas, "{el:?} must be distinct from canvas");
              assert!(contrast_ratio(f.fg.unwrap(), f.bg.unwrap()) >= 4.5 - 0.01,
                  "{el:?} fg clears the legibility floor");
          }
      }
    ```

  **1c. New invariant tests** (make the guarantees explicit — not tied to one theme):
  ```rust
      #[test]
      fn derive_stack_ordered_and_floored_all_rgb_builtins() {
          // (a)+(b): for every RGB builtin, at FULL and ZEN, the four-layer stack is strictly
          // ordered toward the headroom pole AND each adjacent pair clears its CR target.
          for name in Theme::builtin_names() {
              let base = Theme::builtin(name).unwrap();
              if !matches!(base.base_bg, Color::Rgb { .. }) { continue; } // skip terminal-*/no-color
              for (disp, target) in [(ChromeDisposition::Full, 1.30_f32), (ChromeDisposition::Zen, 1.12)] {
                  let mut t = Theme::builtin(name).unwrap();
                  t.derive_chrome(disp);
                  let canvas = t.base_bg;
                  let bar  = t.face(SemanticElement::Chrome).bg.unwrap();
                  let drop = t.face(SemanticElement::ChromeMuted).bg.unwrap();
                  let ov   = t.face(SemanticElement::ChromeOverlay).bg.unwrap();
                  for (below, above, lbl) in [(canvas, bar, "canvas→bar"), (bar, drop, "bar→dropdown"),
                                              (drop, ov, "dropdown→overlay")] {
                      assert!(contrast_ratio(above, below) >= target - 0.01,
                          "{name} {disp:?} {lbl}: CR {} < target {target}",
                          contrast_ratio(above, below));
                  }
              }
          }
      }

      #[test]
      fn derive_full_distinct_from_zen_all_rgb_builtins() {
          // (d): the FULL bar tone and the ZEN bar tone are perceptibly distinct (CR ≥ ~1.14).
          for name in Theme::builtin_names() {
              let base = Theme::builtin(name).unwrap();
              if !matches!(base.base_bg, Color::Rgb { .. }) { continue; }
              let mut f = Theme::builtin(name).unwrap(); f.derive_chrome(ChromeDisposition::Full);
              let mut z = Theme::builtin(name).unwrap(); z.derive_chrome(ChromeDisposition::Zen);
              let fb = f.face(SemanticElement::Chrome).bg.unwrap();
              let zb = z.face(SemanticElement::Chrome).bg.unwrap();
              assert!(contrast_ratio(fb, zb) >= 1.14,
                  "{name}: full≠zen bar CR {} too small", contrast_ratio(fb, zb));
          }
      }

      #[test]
      fn derive_every_chrome_fg_clears_legibility_floor() {
          // (c): every derived chrome fg clears 4.5 vs its own panel, on all RGB builtins.
          for name in Theme::builtin_names() {
              let base = Theme::builtin(name).unwrap();
              if !matches!(base.base_bg, Color::Rgb { .. }) { continue; }
              for disp in [ChromeDisposition::Full, ChromeDisposition::Zen] {
                  let mut t = Theme::builtin(name).unwrap();
                  t.derive_chrome(disp);
                  for el in [SemanticElement::Chrome, SemanticElement::ChromeMuted, SemanticElement::ChromeOverlay] {
                      let f = t.face(el);
                      assert!(contrast_ratio(f.fg.unwrap(), f.bg.unwrap()) >= 4.5 - 0.05,
                          "{name} {disp:?} {el:?} fg CR {} < 4.5", contrast_ratio(f.fg.unwrap(), f.bg.unwrap()));
                  }
              }
          }
      }
  ```
  **Depth coverage (honesty on the spec's "every color depth" wording, Codex plan-gate r1 minor):**
  these ordering/floor/`full≠zen`/fg invariants operate on the **truecolor** derived RGB values
  (pre-quantization) — that is where the ladder is defined and the §II.5 pins live. Lower depths are
  covered elsewhere, not re-asserted here: **Ansi16** by the fixed-table policy tests
  (`ansi16_policy_replaces_derived_chrome`, theme_resolve.rs:423 — derived chrome is replaced by the
  named-color table, so ordering is a table property, not a derived one); **Indexed256** by the
  rewritten rung test (1f below). Do NOT claim depth-agnostic ordering for the truecolor invariant
  tests — the ladder guarantee is a truecolor property that the fixed-table policy re-establishes at
  Ansi16 and that 256-quantization preserves (1f).

  **1d. Update the tokyo idempotence test `derive_fills_only_unset_faces`** (theme.rs:1325) — tokyo's
  chrome is still EXPLICIT at T1 (Part D lands in T3). Keep its shape (explicit faces survive; overlay
  + accent derive; second call is a no-op). Its accent pin (`#16161e/#8fa3ce`) is UNCHANGED. Its
  overlay pin CHANGES (new `next_layer` from the explicit `chrome.bg = #16161e`, since
  `chrome_muted.bg == None`): replace `(0x2f,0x30,0x3a)` with the value the shipped `derive_chrome`
  produces `[verify — intermediate; replaced by §II.5 #4e5071 in T3]`.

  **1e. Re-hex the remaining derivation tests that keep their shape** (§I.9): `derive_preserves_hue_angle`
  (theme.rs:1471) — phosphor-green: pin only the ceiling-invariant **bg** channels from §II.5
  (`Chrome #004000`, `Muted #005400`, `Overlay #006800`) + keep the green-dominant assertion; assert
  the fg is `Some` and green-dominant but do NOT pin its hex (deferred to T4 — see GAP note 2).
  `derive_accent_desaturation_bound` (theme.rs:1456) has no hex pin (asserts `accent_s < seed_s`) and
  should still pass unchanged — run it to confirm.

  **1f. Rewrite the Indexed256 rung test `derived_rungs_distinct_at_256`** (theme.rs:1082 — the 5th
  semantic rewrite, GAP note 4). The OLD test asserts the split ladder COLLAPSES chrome/muted onto the
  canvas index (`chrome collapses onto canvas at 256`, theme.rs:1091/1093/1102). Under unified
  elevation every layer clears a CR floor away from the canvas, so at 256 colours the rungs quantize
  to DISTINCT, ordered-toward-pole indices. Rewrite it to assert non-collapse + ordering, and pin the
  regenerated indices `[verify — probe]`:
  ```rust
      #[test]
      fn derived_rungs_distinct_at_256() {
          // New model: elevated rungs do NOT collapse onto the canvas index — each is distinct and
          // ordered toward the headroom pole. Indices regenerated by quantizing the shipped truecolor
          // rungs at Depth::Indexed256 (probe) — [verify]; BLOCKED-on-uncertainty, never invented.
          for (ctor, label) in [
              (flexoki_dark as fn() -> Theme, "flexoki-dark"),
              (catppuccin_mocha, "mocha"),
              (gruvbox_dark, "gruvbox-dark"),
          ] {
              let mut t = ctor();
              t.derive_chrome(ChromeDisposition::Full);
              let q = |c: Color| quantize(c, Depth::Indexed256);
              let canvas  = q(t.base_bg);
              let chrome  = q(t.face(SemanticElement::Chrome).bg.unwrap());
              let muted   = q(t.face(SemanticElement::ChromeMuted).bg.unwrap());
              let overlay = q(t.face(SemanticElement::ChromeOverlay).bg.unwrap());
              assert_ne!(chrome, canvas,  "{label}: chrome distinct from canvas at 256 (elevated)");
              assert_ne!(muted,  chrome,  "{label}: dropdown distinct from bar at 256");
              assert_ne!(overlay, muted,  "{label}: overlay distinct from dropdown at 256");
              // exact regenerated indices — replace with the probe output, [verify]:
              // e.g. flexoki-dark: canvas 232, chrome <i0>, muted <i1>, overlay <i2> (i0<i1<i2).
          }
      }
  ```
  Use the REAL constructors (`gruvbox_dark()`, not the old `sample_base16()` stand-in) so the pinned
  indices match §II.5's themes. If the probe cannot be run or an index is uncertain, STOP and escalate
  BLOCKED.

  **1g. Update the cross-crate app.rs derived-chrome pins** (GAP note 3). In `app.rs`:
  - `rederive_arm_reresolves` (app.rs:5062; the pins at app.rs:5093/:5104): flexoki-dark FULL Chrome bg
    `#0d0c0c → #2a2828`, ZEN `#0f0e0e → #1e1c1c` (§II.5 final — flexoki is stable), `[verify]`. Update
    the `// §B.3 …` comments to reference the new derivation.
  - `rederive_respects_picker_committed_theme` (app.rs:5120; the pins at app.rs:5150/:5161): tokyo FULL
    ChromeOverlay bg `#2f303a → <T1 intermediate>`, ZEN `#21222d → <T1 intermediate>` `[verify —
    intermediate; replaced by §II.5 #4e5071 / #33354a in T3]`.
  - **`chrome_key_parses_and_derives`** (`theme_resolve.rs:399`; the Zen flexoki-dark Chrome pin at
    ~:409): `#0f0e0e → #1e1c1c` (§II.5 final — flexoki-dark is stable across T1) `[verify]`; update the
    `// §B.3 …` comment (Codex plan r2 — this cross-crate site was unowned).
  - **`preview_derives_zen_chrome_bg_on_flexoki_dark`** (`theme_picker.rs:107` and `:131`): the Zen
    flexoki-dark Chrome pin `#0f0e0e → #1e1c1c` at BOTH sites (§II.5 final) `[verify]`; update the
    assertion/comment text (Codex plan r3 — the last unowned flexoki-dark chrome pin site).

  Run: `cargo test -p wordcartel-core -p wordcartel -- derive_ derived_rungs_distinct_at_256 rederive_`
  → RED (the tests reference the new invariants / pin values the old code does not produce; the base16
  and cross-crate pins fail against the old split ladder).

- [ ] **Step 2 (GREEN): rewrite the constants + `derive_chrome`.** Replace theme.rs:334-345 with:
  ```rust
  // ── Chrome derivation — elevation constants (grounding §II.2, probe-calibrated) ──────
  // full and zen are the SAME elevation algorithm with different adjacent-layer CR targets —
  // guaranteeing full ≠ zen on every theme.
  const SEP_FLOOR_CR:  f32 = 1.12;  // zen  — each layer clears CR ≥ 1.12 vs the layer beneath
  const FULL_STEP_CR:  f32 = 1.30;  // full — each layer clears CR ≥ 1.30 vs the layer beneath
  const FG_FLOOR:      f32 = 4.5;   // each chrome fg clears 4.5 vs its own panel (pole-capped)
  const CHROME_PANEL_S_CAP: f32 = 0.35; // elevated-panel S = min(canvas_S, 0.35); LIGHT canvases only
  const LAYER_L_STEP:  f32 = 0.001; // panel-lightness search granularity — finer than one u8 (1/255)
  const FG_NUDGE_STEP: f32 = 0.002; // fg legibility-nudge granularity — finer than one u8 per channel
  const MUTED_FG_BLEND: f32 = 0.35;   // muted fg seed = blend(base_fg, base_bg, 0.35), then nudged
  const ACCENT_DESAT:   f32 = 0.50;   // accent fg = blend(seed, equal_lum_gray(seed), 0.50)
  const ZEN_ACCENT_EXTRA: f32 = 0.40; // zen: extra blend of the accent fg toward the same gray
  ```
  Replace the body of `derive_chrome` (theme.rs:221-300) with (signature, early-return, sentinel
  guards, and the `Face { .., ..Face::default() }` shape all preserved):
  ```rust
      pub fn derive_chrome(&mut self, disp: ChromeDisposition) {
          let (bgr, bgg, bgb, fgr, fgg, fgb) = match (self.base_bg, self.base_fg) {
              (Color::Rgb { r: bgr, g: bgg, b: bgb }, Color::Rgb { r: fgr, g: fgg, b: fgb }) =>
                  (bgr, bgg, bgb, fgr, fgg, fgb),
              _ => return,
          };
          let base_bg = Color::Rgb { r: bgr, g: bgg, b: bgb };
          let base_fg = Color::Rgb { r: fgr, g: fgg, b: fgb };

          let is_dark = rel_lum(bgr, bgg, bgb) < 0.5;
          // Elevate toward the pole with headroom: white on dark canvases, black on light.
          let pole = if is_dark { (255u8, 255u8, 255u8) } else { (0u8, 0u8, 0u8) };
          let headroom = Color::Rgb { r: pole.0, g: pole.1, b: pole.2 };
          // Panels preserve the canvas hue; saturation is capped on LIGHT canvases ONLY — a
          // uniform cap would wash out phosphor/solarized-dark tint (grounding §II.7).
          let (canvas_h, canvas_s, _canvas_l) = rgb_to_hsl(bgr, bgg, bgb);
          let panel_s = if is_dark { canvas_s } else { canvas_s.min(CHROME_PANEL_S_CAP) };
          // full vs zen = the same algorithm, different adjacent-layer CR target.
          let target = match disp {
              ChromeDisposition::Full => FULL_STEP_CR,
              ChromeDisposition::Zen  => SEP_FLOOR_CR,
          };

          // next_layer — grow a panel from the LIGHTNESS of the layer beneath toward the
          // headroom pole (preserving canvas H and the possibly-capped panel S) until it
          // clears `target` WCAG contrast against that layer. Any step finer than one u8 of
          // lightness lands on the first u8-quantized panel clearing the target (§II.5 pins).
          let next_layer = |beneath: Color, target: f32| -> Color {
              let start_l = match beneath {
                  Color::Rgb { r, g, b } => rgb_to_hsl(r, g, b).2,
                  _ => return beneath,
              };
              let mut l = start_l;
              loop {
                  l = if is_dark { (l + LAYER_L_STEP).min(1.0) } else { (l - LAYER_L_STEP).max(0.0) };
                  let (r, g, b) = hsl_to_rgb(canvas_h, panel_s, l);
                  let cand = Color::Rgb { r, g, b };
                  if contrast_ratio(cand, beneath) >= target { return cand; }
                  if (is_dark && l >= 1.0) || (!is_dark && l <= 0.0) { return cand; }
              }
          };

          // derive_fg — legibility floor (A-D3). Returns `seed` unchanged when it already
          // clears the floor (the common case — chrome text keeps body-text identity); else
          // nudges toward the headroom pole. The floor is capped by the pole-vs-panel max CR
          // so a mid-luminance panel always terminates (at the pole in the worst case).
          let derive_fg = |seed: Color, panel: Color| -> Color {
              let floor = FG_FLOOR.min(contrast_ratio(headroom, panel));
              if contrast_ratio(seed, panel) >= floor { return seed; }
              let mut pct = 0.0f32;
              loop {
                  pct += FG_NUDGE_STEP;
                  let cand = blend(seed, pole, pct);
                  if contrast_ratio(cand, panel) >= floor { return cand; }
                  if pct >= 1.0 { return headroom; }
              }
          };

          // ── Chrome (bar — elevated from the canvas) ──────────────────────────────────
          if self.faces.chrome == Face::default() {
              let bg = next_layer(base_bg, target);
              self.faces.chrome = Face { fg: Some(derive_fg(base_fg, bg)), bg: Some(bg), ..Face::default() };
          }
          let bar_bg = self.faces.chrome.bg.unwrap_or(base_bg);

          // ── ChromeMuted (dropdown — elevated from the bar) ───────────────────────────
          if self.faces.chrome_muted == Face::default() {
              let bg = next_layer(bar_bg, target);
              let muted_seed = blend(base_fg, (bgr, bgg, bgb), MUTED_FG_BLEND);
              self.faces.chrome_muted = Face {
                  fg: Some(derive_fg(muted_seed, bg)), bg: Some(bg), dim: Some(true), ..Face::default()
              };
          }
          let drop_bg = self.faces.chrome_muted.bg.unwrap_or(bar_bg);

          // ── ChromeOverlay (overlay — elevated from the dropdown) ─────────────────────
          if self.faces.chrome_overlay == Face::default() {
              let bg = next_layer(drop_bg, target);
              self.faces.chrome_overlay = Face { fg: Some(derive_fg(base_fg, bg)), bg: Some(bg), ..Face::default() };
          }

          // ── ChromeSelected (inverted highlight — unchanged) ──────────────────────────
          if self.faces.chrome_selected == Face::default() {
              self.faces.chrome_selected = Face { fg: Some(base_bg), bg: Some(base_fg), ..Face::default() };
          }

          // ── ChromeAccent (accent fg on the elevated bar bg — fg path unchanged from E3) ─
          if self.faces.chrome_accent == Face::default() {
              let accent_bg = self.faces.chrome.bg.unwrap_or(base_bg);
              let seed = self.faces.link.fg.unwrap_or(base_fg);
              let gray = equal_lum_gray(seed);
              let mut accent_fg = blend(seed, gray, ACCENT_DESAT);
              if disp == ChromeDisposition::Zen {
                  accent_fg = blend(accent_fg, gray, ZEN_ACCENT_EXTRA);
              }
              self.faces.chrome_accent = Face {
                  fg: Some(accent_fg), bg: Some(accent_bg), bold: Some(true), ..Face::default()
              };
          }
      }
  ```
  Update the `derive_chrome` doc-comment (theme.rs:212-220): the ladder is now unified elevation with
  a separation floor; keep the fresh-instance discipline + the never-derives-`chrome_reverse` contract.
  Run: `cargo test -p wordcartel-core -- derive_ ` → GREEN. If any §II.5/§II.5a pin fails, re-check the
  transcription; if the transcription is exact and it still fails, STOP and escalate BLOCKED (do NOT
  edit the literal or a constant).

- [ ] **Step 3 (GREEN + gates + commit).** Full `cargo test -p wordcartel-core -p wordcartel`
  (this now includes the app.rs cross-crate pins from 1g and the theme.rs 256-rung rewrite from 1f; the
  render.rs tests that call `derive_chrome` for base16/phosphor still pass — their base16 pins are
  stable; the tokyo/phosphor render + theme_resolve.rs Ansi16 pins update in T3/T4). `cargo clippy
  --workspace --all-targets` clean; `cargo build` warning-free. Commit:
  `feat(chrome): unified HSL-lightness elevation ladder with separation floor — replaces the split ladder; regenerate base16 chrome pins`.

---

### Task 2: `SE::Text`-empty rule — empty phosphor's `text` face (Part C)

**Files:**
- Modify: `wordcartel-core/src/theme.rs` (phosphor constructor).
- Add tests: `wordcartel/src/render.rs` (heading-colour pins).

**Interfaces:**
- `phosphor()` `text: s(3)` → `text: Face::default()`. tokyo (`text: Face::default()`, theme.rs:465)
  and `from_base16` (`text: Face::default()`, theme.rs:688) are already empty — confirm, no change.
- Consumer: the render `text_fg_or_base` fallback (render.rs:115) supplies body `base_fg` when a span
  has no composed fg — already shipped (canvas effort). Live-preview headings carry their role fg;
  source mode composes only `[SE::Text]` (render.rs:562/637) → uniform `base_fg`.

- [ ] **Step 1 (RED): heading-colour pins.** Add to `render.rs` tests, role-relative (so they survive
  T4's shade shift). A base16 theme AND phosphor render heading text in the ROLE fg in live preview;
  body text renders `base_fg`; source mode is uniform:
  ```rust
      #[test]
      fn heading_text_carries_role_fg_base16_and_phosphor() {
          use crate::editor::RenderMode;
          // Each: (theme, label). base16 (flexoki-dark) already colours headings; phosphor does so
          // ONLY after Part C empties its `text` face (currently text = shade(3) clobbers the role).
          for (theme, label) in [
              (wordcartel_core::theme::flexoki_dark(), "flexoki-dark"),
              (wordcartel_core::theme::Theme::builtin("phosphor-green").unwrap(), "phosphor-green"),
          ] {
              let mut ed = Editor::new_from_text("# Title\nbody\n", None, (40, 6));
              ed.theme = theme;
              ed.active_mut().view.mode = RenderMode::LivePreview;
              derive::rebuild(&mut ed);
              let buf = render_to_buffer(&mut ed, 40, 6);

              let role_fg = compose::compose(&ed.theme, ed.depth,
                  &[SE::Text, SE::Heading(1)]).fg;
              let base_fg = compose::base_canvas(&ed.theme, ed.depth).fg;
              assert!(role_fg.is_some() && role_fg != base_fg,
                  "{label}: heading role fg must be coloured and distinct from base_fg");
              // heading row (0) carries the role fg …
              assert!((0..40).any(|x| buf[(x, 0u16)].style().fg == role_fg),
                  "{label}: live-preview heading must carry the role fg");
              // … body row (1) carries base_fg (the empty-Text fallback).
              assert!((0..40).any(|x| buf[(x, 1u16)].style().fg == base_fg),
                  "{label}: body text must carry base_fg");

              // source mode: uniform base_fg, no heading colour (render.rs:562/637 compose [Text] only).
              ed.active_mut().view.mode = RenderMode::SourcePlain;
              derive::rebuild(&mut ed);
              let src = render_to_buffer(&mut ed, 40, 6);
              assert!(!(0..40).any(|x| src[(x, 0u16)].style().fg == role_fg),
                  "{label}: source mode must NOT carry the heading role fg");
          }
      }
  ```
  Run: `cargo test -p wordcartel -- heading_text_carries_role_fg` → RED for phosphor (its `text = s(3)`
  clobbers the heading role → heading renders `base_fg`, so `buf[..0..].fg == role_fg` fails).

- [ ] **Step 2 (GREEN): empty phosphor's `text`.** In `phosphor()` (theme.rs:849) change
  `text: s(3),` → `text: Face::default(),`. Leave the `s` closure and every other face unchanged
  (phosphor `base_fg` is still `shade(hue, 3)`, so body text falls back to the same colour it had —
  only headings are freed). Run the new test → GREEN.

- [ ] **Step 3 (confirm + gates + commit).** Run `body_text_carries_theme_fg` (render.rs:2806) and
  `source_mode_no_heading_fg_live_preview_has_heading_fg` (render.rs:1515) — both still pass (flexoki
  /tokyo bodies fall back to `base_fg`; phosphor body unchanged). Full `cargo test -p wordcartel-core
  -p wordcartel`; clippy clean; build warning-free. Commit:
  `feat(theme): SE::Text-empty rule — empty phosphor's text face so headings colour per level (Part C)`.

---

### Task 3: tokyo brought to the standard (Part D)

**Files:**
- Modify: `wordcartel-core/src/theme.rs` (tokyo palette + face block + the tokyo idempotence test).
- Modify: `wordcartel/src/render.rs` (tests only: `tokyo_status_matches_menu_bar`,
  `tokyo_overlay_interior_is_themed`).
- Modify: `wordcartel/src/theme_resolve.rs` (tests only: `ansi16_policy_replaces_derived_chrome` —
  tokyo's Ansi16 expectations change once its chrome becomes a sentinel).
- Modify: `wordcartel/src/app.rs` (tests only: `rederive_respects_picker_committed_theme` — the tokyo
  overlay pins finalize to §II.5, replacing the T1 intermediates).

**Interfaces:**
- Aligns `SEL_BG` (theme.rs:455) `#283457 → #292e42` (Folke `bg_highlight`; feeds `selection.bg` +,
  after Part D, `wrap_guide.fg` — `search_match` moves off it to `bg: YELLOW`).
- **Ansi16 policy site inverts for tokyo:** with tokyo's chrome now a sentinel, the Ansi16 fixed-table
  policy (`resolve_theme`) REPLACES tokyo Chrome/Selected/Muted with the dark-arm named-color table —
  it no longer "leaves explicit faces alone." So tokyo Chrome at Ansi16 becomes `bg: DarkGray,
  fg: White` (identical to the flexoki-dark dark-arm case above it in the test), not the old
  `quantize(PANEL_BG) == Black` / `quantize(FG) == Gray`.
- Eight face changes (spec Part D table) using tokyo's own palette constants (theme.rs:444-456).
- `chrome`/`chrome_selected`/`chrome_muted` → `Face::default()` sentinels (join `chrome_overlay`/
  `chrome_accent`) → all five derived by T1's ladder. `chrome_reverse` stays `reverse: Some(true)`.
- Deletes the now-unused `PANEL_BG` constant (theme.rs:456).

- [ ] **Step 1 (RED): tokyo face pins + the derived-chrome parity pins.** Add a `theme.rs` test pinning
  the eight new face values + `SEL_BG` alignment:
  ```rust
      #[test]
      fn tokyo_standardized_faces() {
          use SemanticElement::*;
          let t = tokyo_night();
          let magenta = Color::Rgb{r:0xbb,g:0x9a,b:0xf7};
          let yellow  = Color::Rgb{r:0xe0,g:0xaf,b:0x68};
          let orange  = Color::Rgb{r:0xff,g:0x9e,b:0x64};
          let comment = Color::Rgb{r:0x56,g:0x5f,b:0x89};
          let blue    = Color::Rgb{r:0x7a,g:0xa2,b:0xf7};
          let bg      = Color::Rgb{r:0x1a,g:0x1b,b:0x26};
          let sel_bg  = Color::Rgb{r:0x29,g:0x2e,b:0x42};   // aligned #292e42
          assert_eq!(t.face(Emphasis).fg, Some(magenta));   assert_eq!(t.face(Emphasis).italic, Some(true));
          assert_eq!(t.face(Strong).fg, Some(yellow));      assert_eq!(t.face(Strong).bold, Some(true));
          assert_eq!(t.face(StrongEmphasis).fg, Some(orange));
          assert_eq!(t.face(StrongEmphasis).bold, Some(true)); assert_eq!(t.face(StrongEmphasis).italic, Some(true));
          assert_eq!(t.face(Strikethrough).fg, Some(comment)); assert_eq!(t.face(Strikethrough).strike, Some(true));
          assert_eq!(t.face(SearchMatch).bg, Some(yellow));  assert_eq!(t.face(SearchMatch).fg, Some(bg));
          assert_eq!(t.face(FrontMatter).fg, Some(orange));  assert_eq!(t.face(FrontMatter).italic, Some(true));
          assert_eq!(t.face(DiagGrammar).underline_color, Some(blue));
          assert_eq!(t.face(WrapGuide).fg, Some(sel_bg));
          assert_eq!(t.face(Selection).bg, Some(sel_bg));
          // chrome faces are now all-None sentinels (pre-derive).
          for el in [Chrome, ChromeSelected, ChromeMuted, ChromeOverlay, ChromeAccent] {
              assert_eq!(t.face(el), Face::default(), "{el:?} sentinel");
          }
          assert_eq!(t.face(ChromeReverse).reverse, Some(true), "chrome_reverse kept");
      }
  ```
  Update `derive_fills_only_unset_faces` (theme.rs:1325) — its premise INVERTS: tokyo's chrome is now
  all-sentinel, so ALL FIVE faces derive (no longer "four survive, two derive"). Rewrite it to assert
  all five derive + `chrome_reverse` is kept + idempotence, and pin tokyo's derived chrome to §II.5
  (FULL): `Chrome #2d2f42/#c0caf5`, `Muted #3d405a/#a8adc4`, `Overlay #4e5071/#c0caf5`,
  `Selected #c0caf5/#1a1b26`, `Accent #2d2f42/#8fa3ce` `[verify]`.
  In `render.rs`, update `tokyo_status_matches_menu_bar` (render.rs:1447): tokyo Chrome now DERIVES →
  the FULL-row parity assertion stays, but the pin `Some(Color::Rgb(0x16,0x16,0x1e))` → `#2d2f42`, and
  the "must be PANEL_BG #16161e" comment/docstring → "derived FULL Chrome #2d2f42". Update
  `tokyo_overlay_interior_is_themed` (render.rs:2673): `#2f303a → #4e5071` (pin + the "§B.3 pin"
  comment).
  In `theme_resolve.rs`, update `ansi16_policy_replaces_derived_chrome` (theme_resolve.rs:458-479):
  replace the stale tokyo assertions — tokyo Chrome is now a sentinel, so the dark-arm fixed table
  applies (like flexoki-dark). Change the comment block (theme_resolve.rs:458-461) to note tokyo chrome
  now DERIVES + follows the dark-arm policy, and replace the `quantize(Chrome.bg) == Black` /
  `quantize(Chrome.fg) == Gray` assertions (theme_resolve.rs:469-479) with
  `r3.theme.face(Chrome).bg == Some(Color::DarkGray)` and `.fg == Some(Color::White)` (the dark-arm
  table values). The sentinel `ChromeOverlay`/`ChromeAccent` assertions (theme_resolve.rs:481-489)
  stay. `[verify]` the dark-arm values against the flexoki-dark case in the same test.
  In `app.rs`, update `rederive_respects_picker_committed_theme` (app.rs:5150/:5161): the T1
  intermediate tokyo overlay pins finalize to §II.5 — FULL `#2f303a`-was → `#4e5071`, ZEN → `#33354a`
  `[verify]`; update the `// §B.3 …` comments.
  Run `cargo test -p wordcartel-core -p wordcartel -- tokyo derive_fills ansi16_policy rederive_respects`
  → RED.

- [ ] **Step 2 (GREEN): apply Part D.** In `tokyo_night()`:
  - `SEL_BG` constant: `rgb(0x28,0x34,0x57)` → `rgb(0x29,0x2e,0x42)` (update the `// #283457` comment).
  - Delete the `PANEL_BG` constant (theme.rs:456) — no remaining consumer after the chrome sentinels.
  - Face edits (theme.rs:466-499):
    ```rust
              emphasis: Face { fg: Some(MAGENTA), italic: Some(true), ..Face::default() },
              strong: Face { fg: Some(YELLOW), bold: Some(true), ..Face::default() },
              strong_emphasis: Face { fg: Some(ORANGE), bold: Some(true), italic: Some(true), ..Face::default() },
              strikethrough: Face { fg: Some(COMMENT), strike: Some(true), ..Face::default() },
              // …
              search_match: Face { bg: Some(YELLOW), fg: Some(BG), ..Face::default() },
              front_matter: Face { fg: Some(ORANGE), italic: Some(true), ..Face::default() },
              diag_grammar:  Face { underline: Some(true), underline_color: Some(BLUE), ..Face::default() },
              wrap_guide: Face { fg: Some(SEL_BG), ..Face::default() },
              // selection unchanged in shape — SEL_BG constant now #292e42:
              selection: Face { bg: Some(SEL_BG), ..Face::default() },
              chrome: Face::default(),
              chrome_selected: Face::default(),
              chrome_muted: Face::default(),
              chrome_overlay: Face::default(),
              chrome_accent: Face::default(),
    ```
    Leave `diag_spelling` (`underline_color: RED`), headings, `code`, `link`, `block_quote`,
    `list_marker`, `thematic_break`, `comment`, `marked_block`, `search_current`, `focus_dim`,
    `fold_marker`, `text` (already empty), `chrome_reverse` unchanged.
  Run → GREEN. Confirm `search_current_wins_over_selection_on_overlap` (render.rs:2536) still passes
  (it overrides `SearchCurrent` to LightGreen; unaffected by `search_match`/`SEL_BG` changes).

- [ ] **Step 3 (gates + commit).** Full `cargo test -p wordcartel-core -p wordcartel` (includes the
  theme_resolve.rs Ansi16 + app.rs overlay-final updates); clippy clean; build warning-free. Commit:
  `feat(theme): tokyo to the completeness standard — coloured roles, SEL_BG align, chrome derived (Part D)`.

---

### Task 4: phosphor shade ceiling 0.92 → 0.78 (Part E)

**Files:**
- Modify: `wordcartel-core/src/theme.rs` (`shade`, plus the phosphor derived-chrome fg pins from T1).

**Interfaces:**
- `shade()` (theme.rs:799-806): lightness range `0.08..=0.92` → `0.08..=0.78` (the `l =` line + its
  comment). `shade(hue,0)` (L=0.08) is unchanged → phosphor `base_bg` unchanged; `shade(hue,3..=5)`
  shift → `base_fg`, headings, link, `selection.fg`, `diag_spelling.underline_color` shift (§II.6).
- Because `base_fg`/`link` shift, the phosphor DERIVED-chrome fg/accent finalize here (see GAP note 2);
  bg channels are ceiling-invariant and were already pinned in T1.

- [ ] **Step 1 (RED): the ceiling pins.** Add a `theme.rs` test asserting `shade(hue,5)` stays hued
  (not near-white) for all five phosphor hues, plus the §II.6 shade values and the affected-role pins:
  ```rust
      #[test]
      fn phosphor_shade_ceiling_keeps_bright_shades_hued() {
          // §II.6: after the 0.78 ceiling, s(5) has a wide channel spread (hued), not near-white.
          let hues = [
              ("green",  Color::Rgb{r:0x33,g:0xff,b:0x33}, Color::Rgb{r:0x8f,g:0xff,b:0x8f}),
              ("amber",  Color::Rgb{r:0xff,g:0xb0,b:0x00}, Color::Rgb{r:0xff,g:0xdc,b:0x8f}),
              ("red",    Color::Rgb{r:0xff,g:0x55,b:0x55}, Color::Rgb{r:0xff,g:0x8f,b:0x8f}),
              ("blue",   Color::Rgb{r:0x55,g:0x99,b:0xff}, Color::Rgb{r:0x8f,g:0xbc,b:0xff}),
              ("purple", Color::Rgb{r:0xcc,g:0x99,b:0xff}, Color::Rgb{r:0xc7,g:0x8f,b:0xff}),
          ];
          for (name, hue, s5) in hues {
              assert_eq!(shade(hue, 5), s5, "{name} s(5) = §II.6 pin");   // [verify]
              // "hued": the max-min channel spread is wide (≥ 96), i.e. NOT washed to near-white.
              if let Color::Rgb { r, g, b } = shade(hue, 5) {
                  let spread = r.max(g).max(b) - r.min(g).min(b);
                  assert!(spread >= 96, "{name} s(5) must stay hued: spread={spread}");
              } else { panic!("non-Rgb"); }
          }
          // base_bg (s0) unchanged by the ceiling; base_fg (s3) shifts to §II.6.
          let green = Color::Rgb{r:0x33,g:0xff,b:0x33};
          assert_eq!(shade(green, 0), Color::Rgb{r:0x00,g:0x29,b:0x00}, "green s0 unchanged");
          assert_eq!(shade(green, 3), Color::Rgb{r:0x00,g:0xff,b:0x00}, "green s3 (base_fg) shifts");
      }
  ```
  Run `cargo test -p wordcartel-core -- phosphor_shade_ceiling` → RED (old ceiling gives s5
  `#8fff8f`? no — old s5 green = `#d6ffd6`, spread 41 < 96 → RED; s3 old = `#2bff2b` ≠ `#00ff00` → RED).

- [ ] **Step 2 (GREEN): apply the ceiling.** In `shade()` (theme.rs:802-803):
  ```rust
      // map level 0..=5 to lightness 0.08..=0.78 (ceiling lowered from 0.92 — Part E, §II.6)
      let l = 0.08 + (level.min(5) as f32 / 5.0) * (0.78 - 0.08);
  ```
  Run → GREEN. The pre-existing `phosphor_shade_ramp_varies_lightness` (theme.rs:1035) still passes
  (`lum(s5) > lum(s0)` holds: green `#8fff8f` vs `#002900`).

- [ ] **Step 3 (GREEN): finalize the phosphor derived-chrome fg pins.** `base_fg`/`link` shifted, so
  the phosphor-green derived chrome fg/accent now match §II.5. Extend `derive_preserves_hue_angle`
  (theme.rs:1471) to pin the full §II.5 phosphor-green FULL values (the bg channels were already
  pinned in T1; now add/confirm the fg channels): `Chrome #004000/#00ff00`, `Muted #005400/#54cd54`,
  `Overlay #006800/#00ff00`, `Accent #004000/#bbf3bb` `[verify]`. Update any phosphor `phosphor_green_theme`
  helper comment (theme.rs:1319) if it names the old `shade(hue,3)`/`shade(hue,5)` values. Run
  `cargo test -p wordcartel-core -- derive_preserves_hue_angle phosphor` → GREEN.

- [ ] **Step 4 (gates + commit).** Full `cargo test -p wordcartel-core -p wordcartel` — confirm the
  render.rs phosphor tests (`phosphor_status_line_carries_hue`, `phosphor_border_cells_carry_no_own_bg`)
  still pass (they assert `is_some()` only). Clippy clean; build warning-free. Commit:
  `fix(theme): cap phosphor shade ceiling 0.92→0.78 so bright shades stay hued (Part E)`.

---

### Task 5: the completeness conformance test (Part B)

**Files:**
- Modify: `wordcartel-core/src/theme.rs` (extend the conformance loop near
  `builtin_names_final_nineteen`, theme.rs:1053).

**Interfaces:**
- Consumes T2/T3/T4 (all 16 RGB builtins must already satisfy the contract). Produces a
  build-time guardrail: a new theme leaving a required role unstyled fails the test.
- The per-face requirement contract (spec Part B, derived from the `from_base16` template,
  theme.rs:687-727): **fg-required** — `Emphasis, Strong, StrongEmphasis, Code, Strikethrough, Link,
  Heading(1..=6), BlockQuote, CodeBlock, ListMarker, ThematicBreak, FrontMatter, Comment, FocusDim,
  FoldMarker, WrapGuide`; **underline_color-required** — `DiagSpelling, DiagGrammar`;
  **highlight-required (bg OR reverse)** — `Selection, MarkedBlock, SearchMatch`; **modifier-required
  (≥1 modifier)** — `SearchCurrent`; **empty** — `Text`; **exempt (derived)** — `Chrome, ChromeReverse,
  ChromeSelected, ChromeMuted, ChromeOverlay, ChromeAccent`.

- [ ] **Step 1 (RED): the conformance test.** Add to `theme.rs` tests. Encode the contract as an
  exhaustive match (no catch-all `_` — the compiler must force any new `SemanticElement`):
  ```rust
      #[derive(Clone, Copy, PartialEq, Eq, Debug)]
      enum FaceReq { FgRequired, UnderlineColorRequired, Highlight, Modifier, Empty, Exempt }

      fn face_requirement(el: SemanticElement) -> FaceReq {
          use SemanticElement::*;
          use FaceReq::*;
          match el {
              Text => Empty,
              Emphasis | Strong | StrongEmphasis | Code | Strikethrough | Link
              | Heading(_) | BlockQuote | CodeBlock | ListMarker | ThematicBreak
              | FrontMatter | Comment | FocusDim | FoldMarker | WrapGuide => FgRequired,
              DiagSpelling | DiagGrammar => UnderlineColorRequired,
              Selection | MarkedBlock | SearchMatch => Highlight,
              SearchCurrent => Modifier,
              Chrome | ChromeReverse | ChromeSelected | ChromeMuted | ChromeOverlay | ChromeAccent => Exempt,
          }
      }

      #[test]
      fn every_rgb_builtin_satisfies_the_completeness_contract() {
          // Part B — over the 16 RGB builtins (terminal-plain/terminal-ansi/no-color are non-Rgb
          // and exempt), every face satisfies ITS requirement type from the spec Part B contract.
          for name in Theme::builtin_names() {
              let t = Theme::builtin(name).unwrap();
              if !matches!(t.base_bg, Color::Rgb { .. }) { continue; }   // skip the 3 non-Rgb themes
              for el in ALL_ELEMENTS {
                  let f = t.face(el);
                  match face_requirement(el) {
                      FaceReq::FgRequired => assert!(
                          f.fg.is_some() && f.fg != Some(Color::Default),
                          "{name} {el:?}: fg-required face has no explicit fg"),
                      FaceReq::UnderlineColorRequired => assert!(
                          f.underline_color.is_some(),
                          "{name} {el:?}: underline_color-required face has none"),
                      FaceReq::Highlight => assert!(
                          f.bg.is_some() || f.reverse == Some(true),
                          "{name} {el:?}: highlight face needs a bg OR reverse"),
                      FaceReq::Modifier => assert!(
                          [f.bold, f.italic, f.underline, f.strike, f.reverse, f.dim]
                              .iter().any(|m| *m == Some(true)),
                          "{name} {el:?}: modifier-required face has no modifier"),
                      FaceReq::Empty => assert_eq!(
                          f, Face::default(), "{name} {el:?}: SE::Text must be empty (Part C)"),
                      FaceReq::Exempt => {} // chrome — supplied by the elevation ladder
                  }
              }
          }
      }
  ```
  Run `cargo test -p wordcartel-core -- every_rgb_builtin_satisfies` → this should PASS if T2/T3/T4 are
  complete. To confirm it is a REAL guardrail (RED-then-GREEN discipline), first run it against the
  tree WITHOUT one of the Part D/E fixes if practical, OR add a temporary negative check: assert the
  contract would FAIL on a hand-built theme with `Emphasis.fg = None` (document the negative check
  inline, then delete it before commit). If the test passes immediately, note in the commit that the
  guardrail is retrospective (all 16 already conform post-T2/T3/T4).

- [ ] **Step 2 (gates + commit).** Full `cargo test -p wordcartel-core -p wordcartel`; clippy clean
  (the new `FaceReq` enum + helper are test-scoped — ensure no dead-code/`clippy` warnings, e.g. the
  enum derives are all used); build warning-free. Commit:
  `test(theme): completeness conformance — every RGB builtin satisfies its per-face requirement (Part B)`.

---

## Whole-branch verification (after T5, before the merge gates)

- `cargo test -p wordcartel-core -p wordcartel` fully green; `cargo clippy --workspace --all-targets`
  clean; `cargo build` warning-free.
- Run the PTY smoke suite (`scripts/smoke/run.sh`) and quote its one-line summary verbatim in the
  pre-merge report (mandatory-run / advisory-pass — a red result is surfaced, not a blocker).
- Fable whole-branch review (cross-task invariants: the chrome ladder holds on all 19 themes at all
  depths; `full ≠ zen`; heading colour per level; tokyo/phosphor render pins; no data-loss/panic
  class) AND a Codex pre-merge GO/NO-GO. Re-run after fixes until clean/GO.
- Merge with `superpowers:finishing-a-development-branch` (`--no-ff` to `main`); verify tests on the
  merged result; delete the branch. Push only when explicitly asked.
