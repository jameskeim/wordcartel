# E3+E4 Chrome Ladder + Theme Lineup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** every chrome cell derives from a coherent tonal ladder (RGB pre-blend, split directions, contrast-clamped); a `full|zen` axis with a live toggle and persistence; the 19-theme lineup (rename + terminal-ansi + ten E4 bundles + flexoki-dark launch default, phosphor `-flat` gone); the render.rs split.

**Architecture:** eight tasks along the spec's four seams — T1 core derivation + the two new faces; T2 core lineup edits (phosphor/from_base16/rename/terminal-ansi); T3 the ten E4 themes; T4 resolve + the axis + Ansi16 policy + launch default; T5 the render split (mechanical); T6 the render rewiring + reported-bug pins; T7 the toggle + persistence; T8 e2e + ship polish. Each task gates green independently.

**Tech Stack:** Rust; wordcartel-core (theme) + wordcartel shell; no new dependencies.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-05-wordcartel-e3e4-chrome-themes-design.md` (CLEAN — Codex ×8 + Fable ×3; ten user ratifications: the seven brainstorm forks + C1-A pre-blend/clamp + I1-A split ladder + I4-A phosphor deletion). Grounding (verbatim sites + probe-generated expected values + the theme tables): `.superpowers/sdd/e3e4-grounding.md` — **§B/§C hex literals are probe/source truth; transcribe byte-for-byte; a failing pin means transcription error first, then BLOCKED with evidence — never adjust a literal to pass. Tables marked `[verify]` get a precondition assert or an implementation-time probe, per the established discipline.**
- **The pinned fractions (grounding §B.1, probe-calibrated — these are THE constants):** `BAR_PCT` dark 0.18 / light 0.035; `OVERLAY_PCT` dark 0.09 / light 0.075; `DEEP_PCT` dark 0.43 / light 0.11; `MUTED_FG_BLEND` 0.35; `ACCENT_DESAT` 0.50; `ZEN_COLLAPSE` 0.35; contrast threshold `min(4.5, own fg-vs-canvas)`.
- **Gates after EVERY commit:** `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean (deny LIVE); `cargo build` warning-free. NO `cargo fmt`; `—` em-dash prose comments; no emoji in code (multibyte test corpora exempt); no catch-all `_` arms on `SemanticElement`/`ChromeDisposition` matches.
- Status copy byte-exact (spec): `"chrome: zen"` / `"chrome: full"` / `"chrome: n/a (cue mode)"` / `"chrome: zen (no effect: {name} has fixed chrome)"` / `"chrome: zen (no effect at 16-color depth)"` (and the `full` twins).
- Line anchors are HEAD (`e9675da`) references from the grounding; locate by quoted code after drift.
- Exclude Cargo.lock drift. Trailers on every commit, verbatim (use `git commit -F -`, quoted 'EOF' heredoc — `!` breaks zsh in double-quoted `-m`):
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: core derivation — `derive_chrome`, the two new faces, the ripple set

**Files:**
- Modify: `wordcartel-core/src/theme.rs` only.

**Interfaces:**
- Produces: `pub enum ChromeDisposition { Full, Zen }` (derive Debug/Clone/Copy/PartialEq/Eq); `SemanticElement::{ChromeOverlay, ChromeAccent}`; `ThemeFaces.{chrome_overlay, chrome_accent}: Face`; `pub fn Theme::derive_chrome(&mut self, disp: ChromeDisposition)` (fills only all-None chrome faces; Rgb-bases-only; five color faces, never chrome_reverse); a private sRGB relative-luminance helper + `contrast_ratio` (pub(crate) or test-reachable); `element_from_key` gains `"chrome_overlay"`/`"chrome_accent"`. T2-T8 consume all of it.

- [ ] **Step 1: the failing core battery.** Transcribe the derivation reference from grounding §B.2 into test expectations FIRST (tests in theme.rs's module; the `assert_face` idiom from grounding §D). The battery (expected hex from §B.3, byte-for-byte):
  - `derive_fills_only_unset_faces` — tokyo-night: all four EXISTING chrome faces byte-identical after `derive_chrome(Full)`; the two NEW faces (chrome_overlay/chrome_accent) now Some with §B.3's tokyo values. Second call → byte-equal everything (the sentinel no-op pin).
  - `derive_split_ladder_directions` — flexoki-dark full: bar bg DARKER than canvas (toward black), overlay bg LIGHTER (toward white), exact §B.3 hex; flexoki-light: bar darker, overlay DEEPER darker, exact hex.
  - `derive_zen_collapses_toward_poles` — flexoki-dark zen rungs strictly between canvas and the full rungs (each toward its own pole), exact §B.3 zen hex.
  - `derive_preserves_hue_angle` — phosphor-green post-derivation (T2 deletes the initializers; HERE construct a synthetic all-None-chrome theme with phosphor's bases): every derived bg has the base's hue angle (via the probe's hsl check reimplemented test-side, or compare against §B.3's phosphor hex directly — the literals ARE the pin).
  - `derive_contrast_clamp_floors_at_zero` — a SYNTHETIC low-contrast theme (fg/bg pair below 4.5 by construction — grounding E.3: no bundled theme trips the min() branch, so the pin is synthetic): derived rungs equal canvas (step floored), no panic, no loop.
  - `derive_skips_non_rgb_bases` — a theme with `Color::Default` bases: byte-untouched by the call.
  - `contrast_ratio_matches_wcag` — the luminance helper against two known WCAG pairs (white/black = 21.0 ± epsilon; §B.3's solarized-light fg/canvas = 4.99 ± 0.02).
  Run: `cargo test -p wordcartel-core -- derive_ contrast_ratio` → FAIL to compile (nothing exists): the RED.

- [ ] **Step 2: the enum + faces + ripple set.** Add the two `SemanticElement` variants (with the spec's role doc comments). THE COMPLETE RIPPLE SET (grounding E.1 — the compiler forces each; NO catch-alls): `ThemeFaces` two fields; `face()` two arms; `face_mut()`/`override_face` dispatch (grounding A.1's face_mut match); `element_from_key` two keys; the ALL_ELEMENTS-style test list (32 → 34); `mono_faces()` gains `chrome_overlay: Face::default(), chrome_accent: modface(None, true, false, false, false, true)` (reverse+bold — the a11y-testable cue shape; ChromeOverlay stays default per the spec's a11y exemption); every OTHER constructor (default/tokyo/phosphor/from_base16) gains the two fields as `Face::default()` sentinels — they DERIVE.

- [ ] **Step 3: the derivation.** Transcribe grounding §B.2 verbatim into theme.rs: the fraction consts (Global Constraints values, named `CHROME_BAR_PCT_DARK` etc. with the calibration comment citing tokyo/mocha/latte); `fn blend(base: Color, pole: (u8,u8,u8), pct: f32) -> Color` (Rgb-only per-channel lerp); `fn srgb_luminance`/`fn contrast_ratio`; `pub fn derive_chrome(&mut self, disp: ChromeDisposition)` — early-return unless both bases Rgb; polarity by canvas luminance (< 0.5 = dark); per-face: skip if not all-None; bars toward black both polarities, overlay toward white (dark) / deeper black (light), muted fg = fg blended toward bg at MUTED_FG_BLEND, selected = bg-on-fg explicit, accent = fg desaturated toward equal-luminance gray at ACCENT_DESAT with bold; zen multiplies every pct by ZEN_COLLAPSE; the clamp loop (shrink pct toward 0 while contrast_ratio(fg, rung) < min(4.5, contrast_ratio(fg, canvas))). Doc comments carry the spec's fresh-instance discipline and the never-derives-chrome_reverse contract.

- [ ] **Step 4: GREEN + gates + commit** — `feat(e3e4): derived chrome ladder — split-direction RGB pre-blend with contrast clamp; ChromeOverlay + ChromeAccent`.

---

### Task 2: core lineup edits — phosphor, from_base16, rename, terminal-ansi

**Files:**
- Modify: `wordcartel-core/src/theme.rs` only.

**Interfaces:**
- Consumes: T1. Produces: phosphor constructors with NO chrome initializers; `from_base16` with chrome/chrome_selected/chrome_muted as sentinels (chrome_reverse KEPT); `default()` returning `name: "terminal-plain"`; `pub fn terminal_ansi() -> Theme`; `builtin()`/`builtin_names()` = the 9-name interim set (terminal-plain, terminal-ansi, no-color, tokyo-night, 5 phosphor — the ten E4 names land in T3); `-flat` names REMOVED from both.

- [ ] **Step 1: failing pins.** `phosphor_chrome_derives_fully` (phosphor-green + derive(Full) → §B.3's phosphor hex — the I4-A ratified new look); `from_base16_chrome_derives` (a probe palette through from_base16 + derive → derived, chrome_reverse still reverse-modifier); `terminal_plain_name_and_faces` (name == "terminal-plain", faces byte-identical to today's default() table — the fn keeps its table, E.7); `terminal_ansi_all_named_colors` (every face fg/bg is a named ANSI color or None — a property walk over the §C terminal-ansi table; NOT monochrome; base Default); `builtin_names_interim_nine` (the count test REWRITTEN from all_thirteen: 9 names, no `-flat`, both terminal-*). RED: constructors unchanged.

- [ ] **Step 2: implement.** Delete phosphor's four chrome initializers (grounding A.1's phosphor block); flip from_base16's three (keep chrome_reverse — grounding A.1); `default()`'s `name: "default"` → `"terminal-plain"` (table untouched); add `terminal_ansi()` transcribed from grounding §C's terminal-ansi table verbatim; update `builtin()`/`builtin_names()` (drop 5 `-flat`, add terminal-ansi, rename default's entry); fix `phosphor_16color_floor` (no flat iteration) and any test naming "default"/"-flat" (grounding A.1's breakage list).

- [ ] **Step 3: GREEN + gates + commit** — `feat(e3e4): lineup core — phosphor chrome derives (I4-A), from_base16 sentinels, terminal-plain rename, terminal-ansi`.

---

### Task 3: the ten E4 themes

**Files:**
- Modify: `wordcartel-core/src/theme.rs` only.

**Interfaces:**
- Consumes: T1/T2. Produces: ten `pub fn` constructors (catppuccin_mocha, catppuccin_latte, flexoki_dark, flexoki_light, gruvbox_dark, gruvbox_light, rosepine_moon, rosepine_dawn, solarized_dark, solarized_light) with §C's palette tables + markdown face mappings, chrome all-sentinel (derived); `builtin()`/`builtin_names()` = the FULL 19.

- [ ] **Step 1: failing pins.** `builtin_names_final_nineteen` (order: terminal-plain, terminal-ansi, no-color, tokyo-night, phosphor ×5, then the ten E4 names as listed in D5); `every_builtin_resolves_at_all_depths` (all 19 × {Truecolor, Indexed256, Ansi16} through quantize — no panic, a smoke sweep); per-theme spot pins for TWO exemplars (catppuccin-mocha: base bg/fg == §C hex, heading-1 face == the §C mapping; flexoki-light ditto) — the other eight are transcription-guarded by the §C tables' `[verify]` discipline: each `[verify]`-marked value gets a `// [verify]d against <source URL> at implementation` comment after the implementer confirms it, and the reviewer spot-checks two more themes at random. `derived_rungs_distinct_at_256` (each dark E4 theme's full bar/overlay quantize to distinct 256 indices per grounding §B.5's table — transcribe its expected indices).

- [ ] **Step 2: implement.** Transcribe all ten constructors from grounding §C byte-for-byte (each table cites its source URL in a comment; keep the rgb() style; markdown faces per §C's mappings; chrome fields NOT set — all-sentinel + the two new fields likewise). Wire into builtin()/builtin_names().

- [ ] **Step 3: GREEN + gates + commit** — `feat(e3e4): the ten E4 builtins — catppuccin/flexoki/gruvbox/rosepine/solarized, chrome fully derived`.

---

### Task 4: resolve + the axis + Ansi16 policy + launch default

**Files:**
- Modify: `wordcartel/src/theme_resolve.rs`, `wordcartel/src/config.rs`, `wordcartel/src/app.rs` (seeding only), `wordcartel/src/editor.rs` (field only).

**Interfaces:**
- Consumes: T1-T3. Produces: `resolve_theme(tc, env, disp: ChromeDisposition)` (EVERY call site + test gains the param — grounding A.2/E.1 enumerates them); `ThemeConfig.chrome: Option<ChromeDisposition>` + RawTheme `chrome: Option<String>` (parse "full"/"zen", warn unknown); the resolve order = base pick/construct → derive_chrome(disp) → user styles → cue glyph → THE ANSI16 POLICY (at Depth::Ansi16 on an Rgb-based theme, overwrite the five color faces with grounding §C's Ansi16 fixed table keyed on the quantized-canvas binary predicate); the "default"→terminal-plain alias warning; the "-flat"→base-phosphor fallback warning; the no-config arm returns `Theme::builtin("flexoki-dark")` (Depth::None still wins first); `Editor.chrome_disposition: ChromeDisposition` seeded in run().

- [ ] **Step 1: failing tests** (theme_resolve.rs's module, grounding §D idiom): `no_config_resolves_flexoki_dark`; `no_color_env_still_wins` (NO_COLOR + empty config → no-color); `default_name_aliases_with_warning` (name="default" → terminal-plain + a warning containing "default"); `flat_name_falls_back_with_warning` (name="phosphor-amber-flat" → phosphor-amber + warning); `chrome_key_parses_and_derives` (chrome="zen" → the flexoki zen §B.3 bar hex on the resolved theme); `unknown_chrome_warns_full`; `ansi16_policy_replaces_derived_chrome` (flexoki-dark @ Ansi16 → bar bg DarkGray fg White ≠ Black canvas — the corrected N1 pin; tokyo @ Ansi16 → its explicit chrome quantized as today); fix `resolve_unknown_name_falls_back_with_warning` (expects terminal-plain now) + every existing resolve test gains the disp param (grounding A.2's list).

- [ ] **Step 2: implement** per the spec's D3/D5 + grounding A.2/A.5: the RawTheme/ThemeConfig field + fold; the alias/fallback mappings INSIDE resolve (builtin() itself stays clean); the Ansi16 overwrite step; the launch-default arm; run()'s seeding (`editor.chrome_disposition = cfg.theme.chrome.unwrap_or(ChromeDisposition::Full);` + pass to the resolve call). new_from_text inits the field `Full`.

- [ ] **Step 3: GREEN + gates + commit** — `feat(e3e4): the chrome axis in resolve — disposition param, Ansi16 policy, aliases, flexoki-dark launch default`.

---

### Task 5: the render split (mechanical)

**Files:**
- Create: `wordcartel/src/render_overlays.rs`; Modify: `wordcartel/src/render.rs`, `wordcartel/src/lib.rs` (mod line).

**Interfaces:**
- Produces: `pub(crate) struct ChromeStyles` (the six precomputed RStyles + two new slots ov_fill/ov_accent — built in render.rs from the T1 faces); `pub(crate) fn render_overlays::paint(frame, editor, &ChromeStyles)` containing the five overlay painters + menu + diag, code MOVED byte-identical except the mechanical style-struct threading. Geometry helpers STAY in render.rs pub(crate) (mouse.rs imports unchanged — grounding A.10).

- [ ] **Step 1:** build `ChromeStyles` in render.rs replacing the six locals (grounding A.3's precompute block; the two new slots compose the T1 faces); move the painter block (:739-1081 region) into render_overlays.rs with ONLY the mechanical renames (locals → struct fields). TWO commits: (a) the struct introduction in place, (b) the move — so the move commit diffs byte-identical-modulo-mechanical and review can verify conservation (H1 discipline; the whole-branch review charges it).
- [ ] **Step 2:** full suite green UNCHANGED (no behavior change; render tests pass as-is). Gates. Commits — `refactor(e3e4): ChromeStyles precompute struct` / `refactor(e3e4): render_overlays.rs — the painters move (byte-identical modulo threading)`.

---

### Task 6: the render rewiring + reported-bug pins

**Files:**
- Modify: `wordcartel/src/render_overlays.rs`, `wordcartel/src/render.rs` (status block + tests).

**Interfaces:**
- Consumes: T5's structure. Produces: every chrome surface on the six-face model per spec D2.

- [ ] **Step 1: failing pins** (render.rs test module; TestBackend cell-inspection idiom from grounding §D):
  - `tokyo_status_matches_menu_bar` (THE reported bug: status row bg == menu bar bg under tokyo-night, both == PANEL_BG).
  - `tokyo_overlay_interior_is_themed` (open palette → every cell inside the overlay rect (row interiors AND gaps) has bg == the derived ChromeOverlay bg; no terminal-default cells).
  - `phosphor_border_cells_carry_no_own_bg` (THE halo: under phosphor-green with the palette open, every border glyph cell's bg == the interior fill bg — both dispositions).
  - `prompt_active_status_uses_accent` (open a minibuffer → status fg == ChromeAccent fg + bold; normal → Chrome).
  - REWRITE `default_status_line_still_reversed` → `terminal_plain_status_carries_chrome_face` (grounding E.4: White/Black explicit, not REVERSED).
  - Update the a11y test (drop phosphor-amber-flat; add ChromeAccent-carries-modifiers under no-color; ChromeOverlay exempted with the rationale comment).
  - Goldens (scrollbar/menu-fill) survive by compose-relative construction — verify and say so.
- [ ] **Step 2: rewire** per spec D2 + grounding A.3: overlay rect fill (set_style ChromeOverlay after Clear) + rows/query compose [ChromeOverlay]; borders fg-only (strip the bg from the border style — the fill shows through, ratatui patch semantics verified); selected rows ChromeSelected; status explicit Chrome + the accent state split; menu/scrollbar roles unchanged (values now derived).
- [ ] **Step 3: GREEN + gates + commit** — `feat(e3e4): every chrome cell themed — overlay fills, fg-only borders, explicit status with accent prompt state`.

---

### Task 7: the toggle + persistence

**Files:**
- Modify: `wordcartel/src/registry.rs`, `wordcartel/src/app.rs`, `wordcartel/src/editor.rs`, `wordcartel/src/settings.rs`, `wordcartel/src/config.rs` (round-trip test).

**Interfaces:**
- Consumes: T4's resolve signature + Editor field. Produces: `toggle_chrome` ("Chrome: Full/Zen", Settings menu, registered BEFORE save_settings — the journey rule); `editor.theme_rederive: bool`; the between-reduces rederive arm (after the save arm, same region); OTheme/MaskTheme.chrome + parse_mask passthrough + the plain diff_key theme-chrome arm + SettingsSnapshot.chrome_disposition + snapshots; the honest status arms.

- [ ] **Step 1: failing tests:** `toggle_chrome_flips_and_requests_rederive` (dispatch → disposition flipped, flag set, status "chrome: zen"); the three honest arms (cue theme → "chrome: n/a (cue mode)", no flip; terminal-plain → flips + "chrome: zen (no effect: terminal-plain has fixed chrome)"; an Rgb theme with editor depth Ansi16 → flips + "chrome: zen (no effect at 16-color depth)") — the handler reads `editor.theme.monochrome` / base-Rgb-ness / `editor.depth`; `rederive_arm_reresolves` (the seam test calling the REAL between-reduces helper — extract `pub(crate) fn rederive_theme_if_requested(editor, cfg_theme, env) -> bool` in app.rs, the established one-source-of-truth shape — assert the applied theme's bar face flips between §B.3's full and zen hex); `chrome_persists_through_the_diff_law` (rules 1/3/mask on the "zen" string via the new plain arm); the config.rs round-trip gains `[theme] chrome = "zen"` + reload assert; membership test gains `("toggle_chrome", "Chrome: Full/Zen")`; `journey_palette_end` still passes (save_settings stays last — verify).
- [ ] **Step 2: implement** per spec D3 + grounding A.6/A.7/A.9 (the update set verbatim: OTheme.chrome, MaskTheme.chrome, parse_mask passthrough beside the sentinel, the diff arm, snapshot_of/runtime_snapshot fields, snap()/round-trip literals gain the field — the per-field pattern from wrap_column, every site enumerated in grounding E.1).
- [ ] **Step 3: GREEN + gates + commit** — `feat(e3e4): toggle_chrome — live full|zen with honest no-effect arms; disposition persists per-field`.

---

### Task 8: e2e + ship polish

**Files:**
- Modify: `wordcartel/src/e2e.rs`, `wordcartel/src/theme_picker.rs` (>= count), `wordcartel/src/app.rs` (preview derivation).

**Interfaces:** consumes everything.

- [ ] **Step 1:** `preview_selected_theme` gains the derivation on the fresh builtin (`theme.derive_chrome(editor.chrome_disposition)` before apply — grounding A.9; apply_theme itself UNTOUCHED); picker pins: `rebuild_rows` count `>= 13` → `>= 19`; a preview-derives pin (open picker under zen, Down to flexoki-dark, the LIVE theme's bar == §B.3's zen hex).
- [ ] **Step 2: the journey** (`journey_chrome_zen_toggle`): under tokyo-night (Harness with the resolved theme — grounding §D's harness idiom), open the palette → assert a themed interior cell via the harness buffer (or delegate to T6's render pin and assert text presence); dispatch toggle_chrome via palette → status "chrome: zen"; Save Settings → the file carries `[theme] chrome = "zen"`.
- [ ] **Step 3: full gates + smoke** (`scripts/smoke/run.sh`, quote VERBATIM — advisory).
- [ ] **Step 4: commit** — `feat(e3e4): preview derivation, 19-theme picker, the zen journey`.

---

## Verification appendix (final gates charge)

- The ten ratifications hold end-to-end; the fraction consts match §B.1 exactly; every §B.3 pin literal byte-true against the shipped code.
- The reported-bug pins pass (status==bar under tokyo; themed interiors; no phosphor halo; the blend you expected in zen).
- The sentinel contract: tokyo's four faces + terminal-*'s tables byte-survive; phosphor/from_base16/E4 fully derive; second-derive no-op.
- The split conservation (T5's move commit) byte-verified; mouse.rs untouched; geometry helpers home unchanged.
- Statuses byte-exact; save_settings still the last registration; no core→shell dependency leaks (ChromeDisposition lives in core).
- Pre-merge: smoke verbatim + a LIVE tmux sanity — flexoki-dark greets a fresh launch; palette interior themed; toggle zen visibly collapses the bars; phosphor-green borders halo-free; Save Settings carries the chrome key; Ansi16 (TERM=xterm) shows DarkGray bars.
- Ship-time bookkeeping: backlog E3+E4 → SHIPPED (+ the settings-panel deferral note stands); working order advances to E1+E2(+A3); memory updated.
