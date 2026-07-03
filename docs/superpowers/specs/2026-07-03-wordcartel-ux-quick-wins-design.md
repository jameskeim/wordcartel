# UX quick-wins bundle (A2 + B3 + C1) — design

**Status:** approved design (pre-spec-review)
**Date:** 2026-07-03
**Effort:** ux-quick-wins — the first effort off `docs/ux-backlog.md`: three settled, small items
bundled into one branch/pipeline pass. Decisions were resolved with the user 2026-07-03 (backlog
"Resolved decisions" #2, #5, #8; C1/B3/A2 sections).

## Context

The UX backlog triage settled three trivial-to-small items with immediate visible payoff:
- **C1:** exports are pandoc-based (`export.rs`) but there is no LaTeX export, `export_pdf`
  silently uses pandoc's default engine (pdflatex — wrong for a xelatex user), and export-time
  typography (pandoc's `smart`, implicitly ON today) is unowned/unconfigurable.
- **B3:** the `█ ▓ ▒ ░ ▏ ·` heading-level glyphs are ON only for `no_color` + phosphor-flat;
  the user wants them everywhere, in each theme's colors (decision: default ON for ALL themes,
  config opt-out).
- **A2:** the menu bar paints only its label rects; gaps + the right side expose the editing
  row's cells (decision: full-width Chrome background fill; NO right-edge content — that is an
  E1 design later).

## Goals

- `export_tex` (standalone, compilable `.tex`), a xelatex default PDF engine, and an owned
  `export.typography` switch — all configurable via a new `[export]` section.
- Heading glyphs default ON in every theme, with the existing config opt-out intact.
- The menu bar row reads as a bar: full-width Chrome fill whenever it renders.

## Non-goals

- NOT A1 (bar modes/dwell) — the bar still renders only while the menu is open; the fill lives
  inside that same conditional and composes with A1 later.
- NO right-edge bar content (E1 decides that).
- NO `export.pdf_variables` (letterpaper/mainfont pass-through) — deferred; YAGNI until asked.
- NO in-editor typography (source-as-is stands); typography applies at export only.

## Component 1 — C1: `export_tex` + xelatex engine + typography config

**Facts (from the surface map):** every export builds a pandoc argv with a literal
`-f markdown` (html: Capture path, `export.rs:131-146`; docx/pdf: WritesOutput temp+rename
path, `export.rs:149-188`, argv at `:160-166`). `sink_for_ext` routes `"tex"` to
`WritesOutput` already (the `_` fallthrough) — no sink change. `-s` is used nowhere today.
No `[export]` config section exists; sections reach commands as `pub` fields seeded onto
`Editor` in `run()` (app.rs:1956-1959 — the `view_opts`/`diag_cfg` precedent). Registry
entries at registry.rs:177-188; `MenuCategory::Export` exists.

### 1a. `ExportConfig`

```rust
// config.rs — resolved section (ViewConfig pattern, config.rs:73-89)
#[derive(Debug, Clone)]
pub struct ExportConfig {
    /// Passed to pandoc as --pdf-engine for PDF export.
    pub pdf_engine: String,      // default: "xelatex"
    /// Export-time smart punctuation (curly quotes, dashes, ellipses).
    /// true → `-f markdown` (pandoc's smart default; today's implicit behavior);
    /// false → `-f markdown-smart` (strict literal). Applies to ALL export formats.
    pub typography: bool,        // default: true
}
```

Plus the `RawExport` deserialize counterpart (all fields `Option<T>`, `#[serde(default)]` —
the RawView pattern, config.rs:200-209) and per-field folding in `load()` (config.rs:288-309
pattern). `Config` (config.rs:33-41) gains `pub export: ExportConfig`; TOML section `[export]`.
`Editor` gains `pub export_cfg: crate::config::ExportConfig` (default in `new_from_text`),
seeded in `run()` beside `view_opts`/`diag_cfg`.

NOTE (default choice, flagged at design review): `pdf_engine` defaults to `"xelatex"` — a
deliberate behavior change from pandoc's pdflatex default, per the resolved backlog decision;
pdflatex users set `[export] pdf_engine = "pdflatex"`.

### 1b. The pure argv seam + per-format args

Extract argv construction into a pure function (unit-testable without spawning pandoc):

```rust
/// Build the pandoc argv for one export. Pure — the testable seam.
fn pandoc_argv(sink: &ExportSink, out: Option<&Path>, cfg: &ExportOpts) -> Vec<String>
```

where `ExportOpts { typography: bool, pdf_engine: String }` (or the two fields threaded
directly — plan's choice). Rules:
- Input format: `-f markdown` when `typography`, `-f markdown-smart` when not — BOTH argv
  sites (Capture html + WritesOutput).
- `"tex"`: append `-s` (`--standalone`) — without it pandoc emits a body fragment, not a
  compilable document. Other formats unchanged (no `-s`).
- `"pdf"`: append `--pdf-engine={engine}` (single `=`-joined arg).
- html/docx argv otherwise byte-identical to today.

`run_export` reads `editor.export_cfg` and passes the resolved opts into the `do_export`
closure → worker thread (extend `run_pandoc`'s signature or carry `ExportOpts`). Status
strings, TOCTOU guard, temp naming (`name.tex.tmp-{pid}`), probe, and `Msg::ExportDone`
handling all unchanged.

### 1c. The command

```rust
r.register("export_tex", "Export LaTeX", Some(MenuCategory::Export), |c| {
    crate::export::run_export(c.editor, "tex", &c.msg_tx);
    CommandResult::Handled
});
```

registered beside the other three (registry.rs:185-188) — appears in the Export menu + the
palette automatically (the three-surface contract).

## Component 2 — B3: heading glyphs default ON in every theme

**Facts:** `Theme.heading_level_glyph: bool` — `no_color()` already `true` (theme.rs:223);
flip `default()` (:232), `tokyo_night()` (:288), `from_base16()` (:349), and `phosphor(...)`
(:536, currently `heading_level_glyph: flat`). The override chain
(`apply_cue_mode_glyph`, theme_resolve.rs:79-81, applied AFTER construction) is confirmed
safe: cue mode still forces `true`; otherwise `cfg_override.unwrap_or(constructor)` — so
`[theme] heading_level_glyph = false` still wins after the flip.

- Set `heading_level_glyph: true` in all four constructors. In `phosphor`, the `flat` param
  then feeds ONLY `monochrome` (confirm no other `flat` consumers — plan-confirm).
- **Two existing tests update to the new expectation:**
  - `theme.rs:615` (`default_base_is_terminal_default`): `assert!(!t.heading_level_glyph)`
    flips to `assert!(t.heading_level_glyph)`.
  - `render.rs:1136-1148` (`renders_concealed_heading_and_cursor_on_active_line`): row 0 now
    begins with the H1 shade prefix (`"█ "`) before `Title` — update the `starts_with`
    expectation to the actual rendered form (plan-confirm the exact string against the real
    render; do not weaken the assertion, re-point it).
- **One new render test:** the DEFAULT theme renders the shade prefix for a heading (pins the
  flipped default, not just `no_color`).
- Rendering gate unchanged (render.rs:412-421 / :477-486); glyphs render in each theme's
  heading face color by construction.

## Component 3 — A2: menu bar full-width fill

**Facts:** the bar renders inside `if let Some(ref menu) = editor.menu` (render.rs:906-941);
`menu_bar_layout` (render.rs:108-118) emits one label-sized Rect per category; gaps + the
right side get no paint. Styles: `menu_closed_style` = `SE::Chrome`, `menu_open_style` =
`SE::ChromeSelected` (render.rs:705-708). Row 0 is reserved only while the menu is open
(render.rs:248-251) — the fill therefore lives in the same conditional and does not
interact with A1's future modes. No existing test reads the bar row; no e2e journey or
smoke check depends on it.

- Before the per-label loop, paint the full row —
  `Rect::new(area.x, area.y, w, 1)` — with `menu_closed_style` (a styled empty `Paragraph`
  or `buffer.set_style`; plan-confirm which fills the background reliably). Labels render
  after and overwrite their rects; the open-category label keeps `ChromeSelected`.
- **One new render test:** with the menu open, every cell of row 0 carries the Chrome
  background style (full-width assertion, not just under labels).

## Testing

- **C1:** unit tests on the pure `pandoc_argv` — tex gets `-s` + infers `.tex` output via
  `-o`; pdf gets `--pdf-engine=xelatex` (and a custom engine from config); typography=false
  flips the input format to `markdown-smart` on BOTH sink paths; html/docx argv otherwise
  identical to today's (pin the exact vectors). Config-fold tests per the existing section
  patterns ([export] absent → defaults; partial section → per-field inherit).
- **B3:** the two updated tests + the new default-theme shade test.
- **A2:** the new full-width row-0 style test.
- Suite green (`cargo test -p wordcartel-core -p wordcartel`), workspace clippy deny-gate
  clean, build/test-compile warning-free. **PTY smoke suite: mandatory-run, advisory-pass**
  — the pre-merge report quotes `scripts/smoke/run.sh`'s one-line summary verbatim (house
  rule, 2026-07-03). No export test spawns pandoc (the argv seam is the test surface).

## Decomposition (3 tasks)

1. **C1 export** — `ExportConfig`/`RawExport`/fold + `Editor.export_cfg` seeding + the
   `pandoc_argv` seam + per-format args (`-s` for tex, `--pdf-engine` for pdf, typography
   format string) + the `export_tex` registry entry + argv/config unit tests.
2. **B3 glyphs** — flip the four constructor defaults (+ `phosphor` `flat`→`monochrome`-only)
   + update the two tests + the new default-theme shade test.
3. **A2 bar fill** — the full-width Chrome fill + the row-0 style test.

## Global constraints

- `cargo test -p wordcartel-core -p wordcartel` green; workspace clippy **deny** gate clean;
  no `cargo fmt`; house style (em-dash `—`); `#![forbid(unsafe_code)]` untouched.
- Smoke suite run + verbatim summary in the pre-merge report (advisory, never a merge gate).
- Behavior changes are exactly the three decided ones (glyph defaults, pdf engine default,
  the bar fill); everything else byte-identical — the argv unit tests pin html/docx.

## Plan-confirms (resolve during the implementation plan, against real source)

1. The exact current argv construction at BOTH sites (Capture `export.rs:131-146`,
   WritesOutput `:160-166`) and the cleanest extraction shape for `pandoc_argv` (+ how the
   opts thread through `do_export`'s closure into `run_pandoc` — extend the signature or an
   `ExportOpts` struct).
2. The `[export]` fold: exact `RawConfig` field + `load()` fold lines to mirror
   (config.rs:288-309), and where `Editor.export_cfg` defaults in `new_from_text`.
3. `phosphor`'s `flat` param: confirm its ONLY remaining consumer is `monochrome` after the
   flip (grep `flat` in theme.rs) and that the flat/non-flat test pair
   (`phosphor_flat_is_monochrome_single_shade`, `phosphor_shaded_distinguishes_by_shade`)
   stays green.
4. The exact new expected row-0 string for `renders_concealed_heading_and_cursor_on_active_line`
   after the flip (run the render; likely `"█ Title…"`) — re-point, never weaken.
5. The reliable full-width fill primitive in ratatui 0.30 (styled empty `Paragraph` vs
   `Buffer::set_style` over the rect) — whichever demonstrably sets bg on all cells.
6. Confirm `-f markdown-smart` is valid pandoc syntax for disabling smart on the markdown
   reader (it is per the pandoc manual; verify the flag lands in the right argv position for
   both sinks).
