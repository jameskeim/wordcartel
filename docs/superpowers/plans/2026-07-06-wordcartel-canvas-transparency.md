# Opaque canvas + transparency toggle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Paint the theme's `base_bg` across the editing area (default), gated by a
`[theme] canvas = "opaque" | "transparent"` toggle, so RGB themes own the page (completing E3)
while transparent-terminal writers can let their background show through.

**Architecture:** Clone the shipped E3 `chrome` plumbing for a new orthogonal `canvas` key — a
`CanvasMode` enum, a `parse_canvas` resolver, an `Editor.canvas` field, a `toggle_canvas`
command, and per-field persistence — plus a single full-edit-band `set_style` fill in `render()`
that replaces the source-mode per-span `base_canvas` special-casing. Render-only: unlike chrome,
the toggle needs no re-derivation.

**Tech Stack:** Rust; wordcartel-core (theme enum) + wordcartel shell (config/resolve/render/
settings/registry); ratatui 0.30. No new dependencies.

> Plan status: CLEAN — Codex plan gate r1 (T3/T4 NO-GO: name-mask Critical + Importants folded) → r2 (GO x4, empty findings), 2026-07-06. Final gate before merge: Fable whole-branch review (per the 2026-07-06 gating change).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-06-wordcartel-canvas-transparency-design.md` (CLEAN —
  Codex gate r1→r2). Grounding (verbatim mirror-code): `.superpowers/sdd/canvas-grounding.md`.
- **The key is orthogonal to `chrome`.** `[theme] canvas = "opaque" | "transparent"`, default
  `opaque`. It does NOT re-derive or re-resolve the theme — it only changes the render's decision
  to paint the edit band and fill modal interiors.
- **Config copy (byte-exact):** unknown-value warning `"theme.canvas: unknown value `{other}` —
  using opaque"`. Toggle status: `"canvas: opaque"` / `"canvas: transparent"`; no-canvas arm
  `"canvas: opaque (no effect: {name} has no canvas)"` / the `transparent` twin
  (`{name}` = `editor.theme.name`).
- **`save_settings` MUST remain the LAST registration** (`journey_palette_end`); `toggle_canvas`
  registers before it (beside `toggle_chrome`).
- **Gates after EVERY commit:** `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy
  --workspace --all-targets` clean (deny is live — use `io::Error::other`, no new warnings);
  `cargo build` warning-free. NO `cargo fmt`; `—` em-dash prose comments; no emoji in code; no
  catch-all `_` on `CanvasMode`/`ChromeDisposition`/`SemanticElement` matches.
- Exclude Cargo.lock drift. Trailers on every commit, verbatim (`git commit -F -`, quoted 'EOF'
  heredoc — `!` breaks zsh in double-quoted `-m`):
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: `CanvasMode` enum + config field + `parse_canvas`

**Files:**
- Modify: `wordcartel-core/src/theme.rs` (add enum after `ChromeDisposition`, ~:46)
- Modify: `wordcartel/src/config.rs` (`ThemeConfig` ~:50, `RawTheme` ~:211, fold ~:450)
- Modify: `wordcartel/src/theme_resolve.rs` (add `parse_canvas` after `parse_chrome` ~:69)

**Interfaces:**
- Produces: `wordcartel_core::theme::CanvasMode { Opaque, Transparent }` (derive
  `Clone, Copy, PartialEq, Eq, Debug`); `wordcartel::theme_resolve::parse_canvas(&Option<String>)
  -> (CanvasMode, Option<String>)`; `config::ThemeConfig.canvas: Option<String>`. T2-T4 consume.

- [ ] **Step 1: Failing test — `parse_canvas`.** In `theme_resolve.rs`'s test module (beside the
  `parse_chrome` tests), add:
```rust
#[test]
fn parse_canvas_maps_values() {
    use wordcartel_core::theme::CanvasMode;
    assert_eq!(parse_canvas(&None), (CanvasMode::Opaque, None));
    assert_eq!(parse_canvas(&Some("opaque".into())), (CanvasMode::Opaque, None));
    assert_eq!(parse_canvas(&Some("transparent".into())), (CanvasMode::Transparent, None));
    let (m, w) = parse_canvas(&Some("bogus".into()));
    assert_eq!(m, CanvasMode::Opaque);
    assert_eq!(w.as_deref(), Some("theme.canvas: unknown value `bogus` — using opaque"));
}
```
- [ ] **Step 2: Run — expect FAIL** (no `CanvasMode`, no `parse_canvas`).
  Run: `cargo test -p wordcartel -- parse_canvas_maps_values`
- [ ] **Step 3: Add the enum** in `wordcartel-core/src/theme.rs` immediately after
  `pub enum ChromeDisposition { Full, Zen }` (:46):
```rust
/// Whether the theme's canvas (`base_bg`) is painted across the editing area.
/// `Opaque` (default) = paint it — RGB themes own the page. `Transparent` = skip it and
/// the modal-interior fill, so a see-through terminal shows through. Render-only; never
/// affects derivation. Non-Rgb `base_bg` (terminal-* themes) has nothing to paint.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CanvasMode { Opaque, Transparent }
```
- [ ] **Step 4: Add `parse_canvas`** in `theme_resolve.rs` after `parse_chrome` (:69), mirroring it:
```rust
/// Parse a `[theme] canvas` config string into a `CanvasMode`.
///
/// `"opaque"` or `None` → `Opaque` (silent). `"transparent"` → `Transparent`.
/// Unknown value → `Opaque` + warning.
pub fn parse_canvas(s: &Option<String>) -> (CanvasMode, Option<String>) {
    match s.as_deref() {
        None | Some("opaque") => (CanvasMode::Opaque, None),
        Some("transparent") => (CanvasMode::Transparent, None),
        Some(other) => (CanvasMode::Opaque,
            Some(format!("theme.canvas: unknown value `{other}` — using opaque"))),
    }
}
```
  Add `CanvasMode` to the `use wordcartel_core::theme::{…}` import at the top of `theme_resolve.rs`
  (beside `ChromeDisposition`).
- [ ] **Step 5: Add the config field.** In `config.rs` `ThemeConfig` (after `chrome` at :50):
```rust
    pub canvas: Option<String>,          // "opaque"|"transparent" — parsed at resolve
```
  In `RawTheme` (after `chrome` at :211): `    canvas: Option<String>,`.
  Add the fold after the `rt.chrome` line (:450):
```rust
        if let Some(c) = rt.canvas { cfg.theme.canvas = Some(c); }
```
- [ ] **Step 6: Run — expect PASS.** `cargo test -p wordcartel -- parse_canvas_maps_values`;
  then the full gates (`cargo test -p wordcartel-core -p wordcartel`, clippy, build).
- [ ] **Step 7: Commit** — `feat(canvas): CanvasMode enum + [theme] canvas config key + parse_canvas`.

---

### Task 2: `Editor.canvas` field, startup seed, `toggle_canvas` command

**Files:**
- Modify: `wordcartel/src/editor.rs` (field after `chrome_disposition` ~:423; init ~:494)
- Modify: `wordcartel/src/app.rs` (seed beside chrome ~:1362-1364)
- Modify: `wordcartel/src/registry.rs` (command + handler, before `save_settings` ~:477)

**Interfaces:**
- Consumes: `CanvasMode`, `parse_canvas` (T1).
- Produces: `Editor.canvas: CanvasMode` (seeded at startup, flipped by `toggle_canvas`). T3 reads
  it in render; T4 reads it in `runtime_snapshot`.

- [ ] **Step 1: Failing test — the toggle + honest arms.** In `registry.rs`'s test module (beside
  the `toggle_chrome` tests) add:
```rust
#[test]
fn toggle_canvas_flips_and_reports() {
    use wordcartel_core::theme::{CanvasMode, Depth};
    // RGB theme at a color depth: flips + plain status.
    let mut ed = crate::editor::Editor::new_from_text("x", None, (40, 4));
    ed.theme = wordcartel_core::theme::Theme::builtin("flexoki-dark").unwrap();
    ed.depth = Depth::Truecolor;
    assert_eq!(ed.canvas, CanvasMode::Opaque);
    toggle_canvas(&mut ed);
    assert_eq!(ed.canvas, CanvasMode::Transparent);
    assert_eq!(ed.status, "canvas: transparent");
    // Non-Rgb theme: flips + persists, honest "no effect".
    let mut ed2 = crate::editor::Editor::new_from_text("x", None, (40, 4));
    ed2.theme = wordcartel_core::theme::Theme::builtin("terminal-plain").unwrap();
    ed2.depth = Depth::Truecolor;
    toggle_canvas(&mut ed2);
    assert_eq!(ed2.canvas, CanvasMode::Transparent, "flip persists even when inert");
    assert_eq!(ed2.status, "canvas: transparent (no effect: terminal-plain has no canvas)");
    // Depth::None (cue) on an Rgb theme: also "no effect" (no color to paint).
    let mut ed3 = crate::editor::Editor::new_from_text("x", None, (40, 4));
    ed3.theme = wordcartel_core::theme::Theme::builtin("flexoki-dark").unwrap();
    ed3.depth = Depth::None;
    toggle_canvas(&mut ed3);
    assert_eq!(ed3.canvas, CanvasMode::Transparent);
    assert_eq!(ed3.status, "canvas: transparent (no effect: flexoki-dark has no canvas)");
}
```
- [ ] **Step 2: Run — expect FAIL** (no `Editor.canvas`, no `toggle_canvas`).
  Run: `cargo test -p wordcartel -- toggle_canvas_flips_and_reports`
- [ ] **Step 3: Add the `Editor.canvas` field.** In `editor.rs` after `chrome_disposition` (:423):
```rust
    /// Canvas opacity (Opaque/Transparent). Seeded from `[theme] canvas` at startup; toggled at
    /// runtime by `toggle_canvas`. Render-only — never re-derives the theme.
    pub canvas: wordcartel_core::theme::CanvasMode,
```
  In `new_from_text` after `chrome_disposition: …::ChromeDisposition::Full,` (:494):
```rust
            canvas: wordcartel_core::theme::CanvasMode::Opaque,
```
- [ ] **Step 4: Seed at startup.** In `app.rs`, right after the chrome seed block
  (`editor.chrome_disposition = chrome_disp;` ~:1364), add:
```rust
    let (canvas_mode, canvas_warn) = crate::theme_resolve::parse_canvas(&cfg.theme.canvas);
    if let Some(w) = canvas_warn { warns.push(w); }
    editor.canvas = canvas_mode;
```
  (The baseline persistence snapshot is handled by `snapshot_of` in Task 4 — do not add a manual
  editor seed there.)
- [ ] **Step 5: Add the `toggle_canvas` handler** in `registry.rs` (beside `toggle_chrome`, before
  its registration). Note: unlike `toggle_chrome`, the flip ALWAYS persists (canvas is a
  cross-theme preference), there is no monochrome early-return, and there is NO `theme_rederive`:
```rust
/// Flip the canvas opacity. Render-only — no re-derive. The flip always persists (canvas is a
/// user preference that outlives the current theme); the status is honest about visibility:
///   • Rgb theme at a color depth: "canvas: opaque"/"canvas: transparent".
///   • non-Rgb base_bg, or Depth::None: flips + persists, "no effect: {name} has no canvas".
fn toggle_canvas(editor: &mut crate::editor::Editor) {
    use wordcartel_core::theme::{CanvasMode, Color, Depth};
    let new_mode = match editor.canvas {
        CanvasMode::Opaque      => CanvasMode::Transparent,
        CanvasMode::Transparent => CanvasMode::Opaque,
    };
    editor.canvas = new_mode;
    let label = match new_mode { CanvasMode::Opaque => "opaque", CanvasMode::Transparent => "transparent" };
    // No canvas to paint: non-Rgb base_bg (terminal-* themes) or the None (cue) depth.
    let has_canvas = matches!(editor.theme.base_bg, Color::Rgb { .. }) && editor.depth != Depth::None;
    if !has_canvas {
        let name = editor.theme.name.clone();
        editor.status = format!("canvas: {label} (no effect: {name} has no canvas)");
        return;
    }
    editor.status = format!("canvas: {label}");
}
```
- [ ] **Step 6: Register the command** in the builder, immediately before the `toggle_chrome`
  registration (:477) — so `save_settings` stays last:
```rust
        r.register("toggle_canvas", "Canvas: Opaque/Transparent", Some(MenuCategory::Settings), |c| {
            toggle_canvas(c.editor);
            CommandResult::Handled
        });
```
- [ ] **Step 7: Run — expect PASS.** `cargo test -p wordcartel -- toggle_canvas_flips_and_reports`;
  then full gates. Confirm `journey_palette_end` (or the save_settings-last membership test) still
  passes.
- [ ] **Step 8: Commit** — `feat(canvas): Editor.canvas field, startup seed, toggle_canvas command with honest arms`.

---

### Task 3: Render — full-edit-band canvas fill + transparent modal interiors

**Files:**
- Modify: `wordcartel/src/render.rs` (`ChromeStyles::build` ~:276 + call site :676; band fill
  after geometry ~:340; source-mode arms :506-522 / :582-595; tests :2228-2272 + new tests)

**Interfaces:**
- Consumes: `Editor.canvas` (T2), `CanvasMode` (T1), `compose::base_canvas` (existing).

- [ ] **Step 1: Failing tests — the band fill + transparent modal.** In `render.rs`'s test module:
```rust
#[test]
fn opaque_canvas_paints_edit_band() {
    use wordcartel_core::theme::{Theme, Depth};
    let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
    ed.theme = Theme::builtin("flexoki-dark").unwrap();
    ed.depth = Depth::Truecolor;
    derive::rebuild(&mut ed);
    let buf = render_to_buffer(&mut ed, 40, 6);
    let want = compose::base_canvas(&ed.theme, ed.depth).bg;   // flexoki-dark base_bg (Rgb)
    assert!(matches!(want, Some(ratatui::style::Color::Rgb(..))), "flexoki base_bg is Rgb");
    // A cell to the RIGHT of the text (col 20, row 0) — never covered by the per-row Paragraph —
    // carries the canvas bg (the blank-area gap the old per-span paint missed).
    assert_eq!(buf[(20u16, 0u16)].style().bg, want, "blank editing cell must carry canvas bg");
    // A below-content editing row (row 3) too.
    assert_eq!(buf[(5u16, 3u16)].style().bg, want, "below-content cell must carry canvas bg");
}

#[test]
fn transparent_canvas_leaves_edit_band_reset() {
    use wordcartel_core::theme::{Theme, Depth, CanvasMode};
    let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
    ed.theme = Theme::builtin("flexoki-dark").unwrap();
    ed.depth = Depth::Truecolor;
    ed.canvas = CanvasMode::Transparent;
    derive::rebuild(&mut ed);
    let buf = render_to_buffer(&mut ed, 40, 6);
    let bg = buf[(20u16, 0u16)].style().bg;
    assert!(bg.is_none() || bg == Some(ratatui::style::Color::Reset),
        "transparent: blank editing cell stays terminal-default; got {bg:?}");
}
```
- [ ] **Step 2: Run — expect FAIL** (`ed.canvas` compiles but the band is unpainted, so the opaque
  test fails on the blank cell). Run: `cargo test -p wordcartel -- opaque_canvas_paints_edit_band transparent_canvas_leaves_edit_band_reset`
- [ ] **Step 3: Insert the band fill** in `render()`, right after the centered-measure geometry
  (`let tg = crate::nav::text_geometry(editor);` at :340) and BEFORE the row loop:
```rust
    // Opaque canvas: fill the whole edit band (margins + blank/below-content rows) with base_bg
    // BEFORE the per-row text Paragraphs — fg-only text preserves it (Cell::set_style patch
    // semantics, same as fg-only borders). Skipped in Transparent mode and when the theme has no
    // canvas to paint (base_bg → Reset, or Depth::None → no color).
    if editor.canvas == wordcartel_core::theme::CanvasMode::Opaque {
        let mut cbg = compose::base_canvas(&editor.theme, editor.depth);
        cbg.fg = None; // bg-only fill
        if cbg.bg.is_some() && cbg.bg != Some(ratatui::style::Color::Reset) {
            let band = Rect::new(area.x, edit_top, w, edit_height);
            frame.buffer_mut().set_style(band, cbg);
        }
    }
```
- [ ] **Step 4: Unify source mode — drop the per-span `base_canvas`.** In the segs path (:506-522)
  and the placed path (:582-595), replace the `source_mode` arms so source text is fg-only
  (`[Text]`, `[Text, FocusDim]` when dim) — the band fill now supplies the bg. segs path:
```rust
                for seg in &vr.segs {
                    let style = if row_dim {
                        if source_mode {
                            compose::compose(&editor.theme, editor.depth, &[SE::Text, SE::FocusDim])
                        } else {
                            compose::compose(&editor.theme, editor.depth, &[SE::Text, role_element(vr.role), style_element(seg.style), SE::FocusDim])
                        }
                    } else if source_mode {
                        compose::compose(&editor.theme, editor.depth, &[SE::Text])
                    } else {
                        compose::compose(&editor.theme, editor.depth, &[SE::Text, role_element(vr.role), style_element(seg.style)])
                    };
                    segs_spans.push(Span::styled(seg.text.clone(), style));
                }
```
  Placed path (:582-595) identically, using `p.style` instead of `seg.style`:
```rust
                    let mut style = if row_dim {
                        if source_mode {
                            compose::compose(&editor.theme, editor.depth, &[SE::Text, SE::FocusDim])
                        } else {
                            compose::compose(&editor.theme, editor.depth, &[SE::Text, role_element(vr.role), style_element(p.style), SE::FocusDim])
                        }
                    } else if source_mode {
                        compose::compose(&editor.theme, editor.depth, &[SE::Text])
                    } else {
                        compose::compose(&editor.theme, editor.depth, &[SE::Text, role_element(vr.role), style_element(p.style)])
                    };
```
- [ ] **Step 5: Add the `CanvasMode` hook to `ChromeStyles::build`** so transparent suppresses the
  modal-interior fill AND the query-bar bg (Codex plan r1 Important — `ov_query` is also an
  overlay interior element). The selected-row highlight and fg-only border stay so the modal
  remains usable. Add `canvas: CanvasMode` as the third param (:276) and restructure the body:
```rust
    pub(crate) fn build(
        theme: &wordcartel_core::theme::Theme,
        depth: wordcartel_core::theme::Depth,
        canvas: wordcartel_core::theme::CanvasMode,
    ) -> Self {
        let transparent = canvas == wordcartel_core::theme::CanvasMode::Transparent;
        // overlay_border: fg-only Chrome — .bg cleared so the fill bg shows through.
        let mut border = compose::compose(theme, depth, &[SE::Chrome]);
        border.bg = None;
        // Overlay interior fills go see-through in transparent mode: ov_fill becomes a no-op and
        // the query bar renders fg-only. overlay_selected keeps its bg (selection stays visible).
        let mut ov_query = compose::compose(theme, depth, &[SE::ChromeOverlay]);
        if transparent { ov_query.bg = None; }
        let ov_fill = if transparent {
            RStyle::default()
        } else {
            compose::compose(theme, depth, &[SE::ChromeOverlay])
        };
        ChromeStyles {
            overlay_selected: compose::compose(theme, depth, &[SE::ChromeSelected]),
            ov_query,
            menu_open:        compose::compose(theme, depth, &[SE::ChromeSelected]),
            menu_closed:      compose::compose(theme, depth, &[SE::Chrome]),
            menu_sel:         compose::compose(theme, depth, &[SE::ChromeSelected]),
            menu_norm:        compose::compose(theme, depth, &[SE::ChromeMuted]),
            scrollbar_track:  compose::compose(theme, depth, &[SE::ChromeMuted]),
            scrollbar_thumb:  compose::compose(theme, depth, &[SE::Chrome]),
            ov_fill,
            ov_accent:        compose::compose(theme, depth, &[SE::ChromeAccent]),
            overlay_border:   border,
        }
    }
```
  Update the call site (:676): `let cs = ChromeStyles::build(&editor.theme, editor.depth, editor.canvas);`.
- [ ] **Step 6: Overlay-fill hook, content-highlight, bars, non-Rgb tests.** Add:
```rust
#[test]
fn transparent_suppresses_overlay_interior() {
    // Modal interiors go see-through in transparent mode: ov_fill is a no-op and the query bar
    // renders fg-only (bg stripped). The selected-row highlight keeps its bg (stays visible).
    // Tested directly on the hook — no palette/registry setup needed.
    use wordcartel_core::theme::{Theme, Depth, CanvasMode, ChromeDisposition};
    let mut theme = Theme::builtin("flexoki-dark").unwrap();
    theme.derive_chrome(ChromeDisposition::Full);
    let opaque = ChromeStyles::build(&theme, Depth::Truecolor, CanvasMode::Opaque);
    let transp = ChromeStyles::build(&theme, Depth::Truecolor, CanvasMode::Transparent);
    assert!(opaque.ov_fill.bg.is_some(), "opaque overlay fill carries a ChromeOverlay bg");
    assert_eq!(transp.ov_fill, RStyle::default(), "transparent overlay fill is a no-op");
    assert!(opaque.ov_query.bg.is_some(), "opaque query bar carries a bg");
    assert!(transp.ov_query.bg.is_none(), "transparent query bar bg is stripped (fg-only)");
    assert!(transp.overlay_selected.bg.is_some(), "selected-row highlight stays visible in transparent");
}

#[test]
fn transparent_keeps_content_highlights() {
    // Content highlights (selection/search/code/diagnostics) keep their explicit bg in
    // transparent mode — canvas mode only touches the band fill + ov_fill, never content
    // composition. A transparent selection would be an invisible selection (spec D1 boundary).
    use wordcartel_core::theme::{Theme, Depth, CanvasMode};
    use wordcartel_core::selection::Selection;
    let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
    ed.theme = Theme::builtin("flexoki-dark").unwrap();
    ed.depth = Depth::Truecolor;
    ed.canvas = CanvasMode::Transparent;
    ed.active_mut().document.selection = Selection::range(0, 2); // select "hi"
    derive::rebuild(&mut ed);
    let buf = render_to_buffer(&mut ed, 40, 6);
    let bg = buf[(0u16, 0u16)].style().bg;
    assert!(matches!(bg, Some(ratatui::style::Color::Rgb(..))),
        "selection highlight must survive transparent canvas; got {bg:?}");
}

#[test]
fn transparent_keeps_bars_painted() {
    use wordcartel_core::theme::{Theme, Depth, CanvasMode};
    let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
    ed.menu_bar_mode = crate::config::MenuBarMode::Pinned;
    ed.theme = Theme::builtin("flexoki-dark").unwrap();
    ed.depth = Depth::Truecolor;
    ed.canvas = CanvasMode::Transparent;
    derive::rebuild(&mut ed);
    let buf = render_to_buffer(&mut ed, 40, 6);
    let menu = compose::compose(&ed.theme, ed.depth, &[SE::Chrome]).bg;
    assert_eq!(buf[(0u16, 0u16)].style().bg, menu, "menu bar stays painted in transparent mode");
    assert_eq!(buf[(0u16, 5u16)].style().bg, menu, "status bar stays painted in transparent mode");
}

#[test]
fn non_rgb_theme_canvas_moot_both_modes() {
    use wordcartel_core::theme::{Theme, Depth, CanvasMode};
    for mode in [CanvasMode::Opaque, CanvasMode::Transparent] {
        let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
        ed.theme = Theme::builtin("terminal-plain").unwrap();
        ed.depth = Depth::Truecolor;
        ed.canvas = mode;
        derive::rebuild(&mut ed);
        let buf = render_to_buffer(&mut ed, 40, 6);
        let bg = buf[(20u16, 0u16)].style().bg;
        assert!(bg.is_none() || bg == Some(ratatui::style::Color::Reset),
            "terminal-plain has no canvas — {mode:?} editing cell stays terminal-default; got {bg:?}");
    }
}

#[test]
fn opaque_canvas_at_ansi16_paints_quantized_bg() {
    use wordcartel_core::theme::{Theme, Depth};
    let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
    ed.theme = Theme::builtin("flexoki-dark").unwrap();
    ed.depth = Depth::Ansi16;
    derive::rebuild(&mut ed);
    let buf = render_to_buffer(&mut ed, 40, 6);
    let want = compose::base_canvas(&ed.theme, Depth::Ansi16).bg;
    assert!(want.is_some() && want != Some(ratatui::style::Color::Reset),
        "flexoki base_bg quantizes to a named Ansi16 color; got {want:?}");
    assert_eq!(buf[(20u16, 0u16)].style().bg, want, "opaque Ansi16 paints the quantized canvas bg");
}

#[test]
fn opaque_canvas_at_depth_none_paints_nothing() {
    use wordcartel_core::theme::{Theme, Depth};
    let mut ed = Editor::new_from_text("hi\n", None, (40, 6));
    ed.theme = Theme::builtin("flexoki-dark").unwrap();
    ed.depth = Depth::None;                       // cue/monochrome — base_canvas has no color
    derive::rebuild(&mut ed);
    let buf = render_to_buffer(&mut ed, 40, 6);
    let bg = buf[(20u16, 0u16)].style().bg;
    assert!(bg.is_none() || bg == Some(ratatui::style::Color::Reset),
        "Depth::None: band guard skips the fill; got {bg:?}");
}
```
- [ ] **Step 7: Re-verify the two source-mode tests.** `source_mode_tints_canvas_for_phosphor_but_not_default`
  (:2228) and `source_mode_dimmed_row_keeps_phosphor_canvas` (:2252) should now pass via the band
  fill (phosphor-amber is Rgb + default Opaque → the band paints cell (0,0)'s bg; terminal-plain
  stays Reset; the dimmed row keeps the band bg + DIM modifier). Run them; if the mechanism change
  shifted which cell carries the bg, adjust the asserted coordinate — do NOT weaken the assertion.
- [ ] **Step 8: Run — expect PASS** for all T3 tests + the two source-mode tests; then full gates
  (`cargo test -p wordcartel-core -p wordcartel`, clippy, build) and `scripts/smoke/run.sh`
  (quote its one-line summary verbatim in the report).
- [ ] **Step 9: Commit** — `feat(canvas): full-edit-band canvas fill + transparent modal interiors; unify source-mode paint`.

---

### Task 4: Per-field persistence (mirror `chrome`)

**Files:**
- Modify: `wordcartel/src/settings.rs` (`SettingsSnapshot` ~:46, `OTheme` ~:91, `snapshot_of`
  ~:150, `runtime_snapshot` ~:167, `MaskTheme`/`parse_mask` ~:206-232, `compute_overrides` diff
  ~:296-312, `snap` test helper ~:482)
- Modify: `wordcartel/src/config.rs` (round-trip test literal ~:842 + assertion ~:872)

**Interfaces:**
- Consumes: `CanvasMode` (T1), `Editor.canvas` (T2), `cfg.theme.canvas` (T1), `parse_canvas` (T1).

- [ ] **Step 1: Failing test — the diff law.** In `settings.rs`'s test module (beside
  `chrome_persists_through_the_diff_law`) add:
```rust
#[test]
fn canvas_persists_through_the_diff_law() {
    use wordcartel_core::theme::CanvasMode;
    // Rule 1: runtime Transparent vs baseline Opaque → diff writes "transparent".
    let mut rt = snap("cua", ThemeIdentity::Builtin("default".into()), false);
    rt.canvas = CanvasMode::Transparent;
    let base = snap("cua", ThemeIdentity::Builtin("default".into()), false);
    let of = compute_overrides(&rt, &base, &OverridesFile::default(), &OverridesFile::default());
    assert_eq!(of.theme.as_ref().and_then(|t| t.canvas.as_deref()), Some("transparent"),
        "rule 1 writes canvas=transparent on divergence");
    // Rule 3: runtime back to Opaque, existing has "transparent" → removes (contradiction, unmasked).
    let rt2 = snap("cua", ThemeIdentity::Builtin("default".into()), false); // Opaque
    let existing = parse_overrides("[theme]\ncanvas='transparent'\n");
    let of2 = compute_overrides(&rt2, &base, &existing, &OverridesFile::default());
    assert!(of2.theme.as_ref().and_then(|t| t.canvas.as_ref()).is_none(),
        "rule 3 removes the contradicted canvas key");
    // Mask-guard: --config [theme] canvas=... guards the key independently.
    let mask = parse_mask("[theme]\ncanvas='transparent'\n");
    let of3 = compute_overrides(&rt2, &base, &existing, &mask);
    assert_eq!(of3.theme.as_ref().and_then(|t| t.canvas.as_deref()), Some("transparent"),
        "mask-guard keeps verbatim when canvas key present in mask");
}
```
- [ ] **Step 2: Run — expect FAIL** (`SettingsSnapshot` has no `canvas`; `OTheme` has no `canvas`).
  Run: `cargo test -p wordcartel -- canvas_persists_through_the_diff_law`
- [ ] **Step 3: Add the fields.** `SettingsSnapshot` (after `chrome_disposition` :46):
```rust
    /// Canvas opacity persisted as "opaque"/"transparent". Per-field — independent of name/chrome.
    pub canvas: CanvasMode,
```
  (Add `use wordcartel_core::theme::CanvasMode;` beside the `ChromeDisposition` import at :9.)
  `OTheme` (after `chrome` :91):
```rust
    #[serde(skip_serializing_if = "Option::is_none")] pub canvas: Option<String>,
```
- [ ] **Step 4: Feed the snapshots.** `snapshot_of` (:150, after `chrome_disposition,`):
```rust
        canvas: crate::theme_resolve::parse_canvas(&cfg.theme.canvas).0,
```
  `runtime_snapshot` (:167, after `chrome_disposition: editor.chrome_disposition,`):
```rust
        canvas: editor.canvas,
```
- [ ] **Step 5: `parse_mask` + the diff arm.** In `MaskTheme` (after `chrome` :213):
`        canvas: Option<String>,`. In the `and_then` closure (:219-226), pass canvas through beside
  chrome as its own predicate:
```rust
    let theme = mask.theme.and_then(|t| {
        let name_file = t.name.is_some() || t.file.is_some();
        let chrome = t.chrome;
        let canvas = t.canvas;
        if name_file || chrome.is_some() || canvas.is_some() {
            Some(OTheme {
                name: if name_file { Some(String::new()) } else { None },
                chrome,
                canvas,
            })
        } else {
            None
        }
    });
```
  In `compute_overrides`, after the chrome diff arm (:310), add the canvas arm:
```rust
    let rt_canvas = match runtime.canvas {
        CanvasMode::Opaque      => "opaque".to_string(),
        CanvasMode::Transparent => "transparent".to_string(),
    };
    let base_canvas_s = match baseline.canvas {
        CanvasMode::Opaque      => "opaque".to_string(),
        CanvasMode::Transparent => "transparent".to_string(),
    };
    let canvas = diff_key(
        &rt_canvas, &base_canvas_s,
        existing.theme.as_ref().and_then(|t| t.canvas.as_ref()),
        mask.theme.as_ref().and_then(|t| t.canvas.as_ref()).is_some(),
    );
```
  and extend the `has_theme` + `OTheme` lift (:311-312) to include canvas:
```rust
    let has_theme = theme_name.is_some() || chrome.is_some() || canvas.is_some();
    let theme = some_if(OTheme { name: theme_name, chrome, canvas }, has_theme);
```
- [ ] **Step 5b: Fix the name-mask independence** (Codex plan r1 Critical — a latent E3 bug the
  canvas key would widen). The name provenance guard at `settings.rs:287` is
  `let theme_masked = mask.theme.is_some();` — but `parse_mask` populates `mask.theme` for a
  chrome-only OR canvas-only mask too, so ANY interior-key mask wrongly shields the theme NAME
  from Rule-3 removal. Narrow the guard to the name sentinel (parse_mask sets `name: Some("")` for
  name/file masks, `None` for interior-only ones):
```rust
    // Name/file provenance only — a chrome/canvas-only --config mask must NOT shield the name
    // (each interior key guards itself via its own diff_key predicate below).
    let theme_masked = mask.theme.as_ref().and_then(|t| t.name.as_ref()).is_some();
```
  Update the stale comment at :283-286 accordingly. Add the independence test:
```rust
#[test]
fn interior_key_mask_does_not_shield_name() {
    // A canvas-only (or chrome-only) --config mask must NOT protect a contradicted name key.
    let rt = snap("cua", ThemeIdentity::Builtin("terminal-plain".into()), false);
    let base = snap("cua", ThemeIdentity::Builtin("terminal-plain".into()), false);
    let existing = parse_overrides("[theme]\nname='tokyo-night'\n"); // stale, now contradicted
    let mask = parse_mask("[theme]\ncanvas='transparent'\n");        // canvas-only mask
    let of = compute_overrides(&rt, &base, &existing, &mask);
    assert!(of.theme.as_ref().and_then(|t| t.name.as_ref()).is_none(),
        "canvas-only mask must not shield the contradicted name key");
}
```
- [ ] **Step 6: Update every other `SettingsSnapshot` constructor** the compiler flags (Codex plan
  r1 Important — do NOT rely on the list being complete; `cargo build` enumerates them). The known
  sites: the `snap` and `empty_snap` test helpers (settings.rs — add `canvas: CanvasMode::Opaque`); the
  `config.rs` round-trip literal (:842-854, add `canvas: CanvasMode::Transparent` AND a
  `use wordcartel_core::theme::CanvasMode;` in that test — currently absent); and the **e2e.rs
  `SettingsSnapshot` literal (e2e.rs:701)** which HEAD also has. Import `CanvasMode` in each test
  module that names it. Extend the config round-trip: after the `chrome` assertion (:872) add
  `assert_eq!(cfg.theme.canvas.as_deref(), Some("transparent"), "[theme] canvas must round-trip");`.
- [ ] **Step 7: Run — expect PASS.** `cargo test -p wordcartel -- canvas_persists_through_the_diff_law save_reload_roundtrip_restores_settings`; then full gates.
- [ ] **Step 8: Commit** — `feat(canvas): per-field persistence — SettingsSnapshot/OTheme/MaskTheme/diff arm + round-trip`.

---

## Notes for the executor

- The effort clones `chrome`; the grounding file `.superpowers/sdd/canvas-grounding.md` has the
  exact chrome originals beside each mirror-site if a snippet's surrounding context is unclear.
- The one load-bearing assumption (a viewport `set_style` survives fg-only `Paragraph` text) is
  Codex-confirmed against this ratatui version; `opaque_canvas_paints_edit_band` (T3) is its live
  proof — if it fails, that assumption is the first suspect.
- `save_settings` last-registration is an invariant — verify `journey_palette_end` after T2.
