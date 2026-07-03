# UX quick-wins bundle (A2 + B3 + C1) — design

**Status:** spec-review CLEAN (Codex x3 + Fable5 folded, bug confirmed empirically); ready for user review + planning
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
pattern). Export uses SIMPLE per-field override folding — none of the accumulation semantics
the keymap (patch lists) or theme (file/name discrimination) sections carry (Codex round 1).
`Config` (config.rs:33-41; currently exactly six sections — export is the seventh) gains
`pub export: ExportConfig`; TOML section `[export]`.
`Editor` gains `pub export_cfg: crate::config::ExportConfig` (default in `new_from_text`),
seeded in `run()` beside `view_opts`/`diag_cfg`.

NOTE (default choice, flagged at design review): `pdf_engine` defaults to `"xelatex"` — a
deliberate behavior change from pandoc's pdflatex default, per the resolved backlog decision;
pdflatex users set `[export] pdf_engine = "pdflatex"`.

### 1b. The pure argv seam + per-format args + the temp-naming fix

**Temp-naming bug — CONFIRMED BROKEN TODAY (Codex round 1 found it; Fable5 verified
empirically against pandoc 3.9.0.2):** the WritesOutput temp is `{target_file_name}.tmp-{pid}`
(export.rs:152) — e.g. `notes.pdf.tmp-1234`, whose EXTENSION is `.tmp-1234`. Live run:
`pandoc -f markdown -o notes.pdf.tmp-1234` warns "Could not deduce format … Defaulting to
html", **exits 0, and writes an HTML fragment** — which `run_pandoc` (checks only
`tmp.exists()`, export.rs:181) and `apply_export_done` (app.rs:387-397) then rename and
report as `"exported notes.pdf"`. **Today's docx/pdf exports silently produce HTML-fragment
bytes under a .docx/.pdf name with a success status** — the project's worst failure class.
C1 is therefore a BUG FIX, and the merge report must state it. Fix: **extension-preserving
temp naming** — `{stem}.tmp-{pid}.{ext}` (e.g. `notes.tmp-1234.pdf`) — REQUIRED for pdf (no
`-t pdf` exists; PDF is triggered only by the `.pdf` extension) and sound for tex/docx.
Belt-and-braces: also pass explicit `-t latex` on the tex path. Fixed shape verified live:
`notes.tmp-1234.docx` → real Word 2007+ file; `notes.tmp-1234.pdf` + `--pdf-engine=xelatex`
→ real PDF 1.7. Nothing globs/cleans `*.tmp-*` by the old pattern (grep-verified — Fable5).

**Composition seam (Fable5 plan-review I-1, user decision A):** a thin pure
`writes_output_invocation(target, ext, pid, opts) -> (PathBuf, Vec<String>)` composes
`temp_path_for` + `pandoc_argv` on the SAME path; `run_pandoc`'s WritesOutput arm consumes the
pair, and one test asserts the argv's `-o` element equals the returned tmp (+ the tmp carries
the format extension). `pandoc_argv` itself still never constructs a path — this guards the
composition (the exact bug class C1 fixes) without violating that contract.

Extract argv construction into a pure function (unit-testable without spawning pandoc):

```rust
/// Build the pandoc argv for one export. Pure — the testable seam. `out` is the
/// ALREADY-DERIVED temp path (None for the Capture/html sink); the temp path is
/// constructed by the caller, not inside this function (Codex round 1).
fn pandoc_argv(sink: &ExportSink, out: Option<&Path>, opts: &ExportOpts) -> Vec<String>
```

where `ExportOpts { typography: bool, pdf_engine: String }`. Rules:
- Input format: `-f markdown` when `typography`, `-f markdown-smart` when not — BOTH argv
  sites (Capture html + WritesOutput).
- `"tex"`: append `-s` (`--standalone`) — without it pandoc emits a body fragment, not a
  compilable document — plus `-t latex` (explicit, no reliance on inference). Other formats:
  no `-s`.
- `"pdf"`: append `--pdf-engine={engine}` (single `=`-joined arg).
- html/docx argv otherwise identical to today MODULO the temp-naming fix above.

**Opts threading (Fable5 IMPORTANT-2 — TWO call sites, not one):** `do_export` is called by
`run_export` AND directly by `resolve_prompt`'s `OverwriteExport` arm (app.rs:699 — the
overwrite-confirmation path bypasses `run_export`). Cleanest shape: **`do_export` reads
`editor.export_cfg` itself** (it already takes `&mut Editor`), building `ExportOpts` there —
both callers stay signature-stable and can never diverge. (Semantics are safe either way: no
runtime config reload exists, so confirm-time config ≡ prompt-time config.) The opts then ride
the closure into the worker → `run_pandoc`. Status strings, TOCTOU guard, probe, and
`Msg::ExportDone` handling all unchanged.

**Pandoc CLI semantics — RESOLVED empirically (Fable5, pandoc 3.9.0.2 + TeX Live; formerly
plan-confirm 6):** `-f markdown-smart` disables smart punctuation on html AND docx, and is the
COMPLETE story for LaTeX/PDF too (with reader-smart off, the latex writer escapes literal
`--` as `-\/-` and `'` as `\textquotesingle` — byte-identical to `-t latex-smart`; literal
hyphens survive TeX ligatures, so typography=false is strict through the whole PDF pipeline).
`--pdf-engine=xelatex` is accepted; a bad engine exits 6 with a whitelist message → surfaces
via `FilterError::NonZero` on the status line (config validation delegated to pandoc is
sound). `-s -t latex` yields a standalone `\documentclass` document that compiles clean under
xelatex; without `-s`, a fragment.

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

- Set `heading_level_glyph: true` in all four constructors. In `phosphor`, `flat` keeps its
  OTHER consumers (the face-branch selection at theme.rs:502-507 and `monochrome`) — only its
  feed into `heading_level_glyph` is removed (Codex round 1 corrected the earlier
  "only monochrome" wording).
- **This is a GEOMETRY change, not render-only (Codex round 1 — owned as part of the decided
  behavior change):** `heading_level_glyph` also drives core layout — inactive headings get
  `prefix_width = 2` when the flag is true (layout.rs:250-255) — and it is threaded through
  the `LayoutKey` cache key (derive.rs:19, :243-267) and nav's layout calls (nav.rs:67, :148).
  Flipping the default therefore shifts wrap positions, caret columns, and click mapping for
  inactive headings under default/tokyo_night/base16 — inherent to showing a two-cell glyph,
  and exactly what was decided. The cache key already contains the flag, so no staleness
  class is introduced. Consequence for the plan: the test sweep is BROADER than the two named
  tests — every nav/caret/layout/render expectation that renders an inactive heading under
  those themes may legitimately shift and must be RE-POINTED to the new correct value (never
  weakened); the e2e fold journey is confirmed safe (`screen_contains("Head")` still matches
  `"█ Head"`, e2e.rs:201), and layout.rs:683's prefix-geometry tests (flag-on) stay green.
- **The sweep is BOUNDED (Fable5 — favorable structural fact):** the glyph placeholder feeds
  `VisualRow.prefix_glyph` (layout.rs:336), NOT `display` — so `display`-string assertions on
  inactive headings survive (derive.rs:398, commands.rs:1191/:1201 stay green). What shifts:
  `Placed.col` (+2), wrap capacity (−2), and rendered buffer row strings. Realistic breakage =
  the two named tests + at most a handful of exact-row-string/column assertions. Cursor
  placement is IMMUNE (`screen_pos` uses the caret line's ACTIVE layout, nav.rs:65; inactive
  layouts key on the same flag for both render and click mapping — no desync class). The
  active↔inactive 2-cell shift when focus enters/leaves a heading is PRE-EXISTING behavior
  (conceal already shifts those lines more) — not a new asymmetry.
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
  `Rect::new(area.x, area.y, w, 1)` — with `menu_closed_style`, using
  **`Buffer::set_style` over the rect** (MANDATED — Codex round 1: ratatui 0.30's `Paragraph`
  merely calls `buf.set_style(area, style)` internally, so the direct primitive is the
  reliable choice). Labels render after and overwrite their rects; the open-category label
  keeps `ChromeSelected`.
- **One new render test (Fable5 IMPORTANT-1 — the naive form is IMPOSSIBLE):** "every cell
  carries Chrome" cannot pass — the menu always has an open category (`menu.open: usize`)
  whose label renders `ChromeSelected` by design (default theme: Chrome bg=Black vs
  ChromeSelected bg=White). The correct assertion: **every row-0 cell OUTSIDE the label rects
  carries the Chrome background, AND no row-0 cell carries the base/unpainted background**
  (equivalently: full-width Chrome-or-ChromeSelected). Never weaken this into an `any()` probe
  — it genuinely fails today (gaps + right side are unpainted).

## Testing

- **C1:** unit tests on the pure `pandoc_argv` — tex asserts `-s -t latex … -o
  {stem}.tmp-{pid}.tex` (explicit format, extension-preserving temp); pdf gets
  `--pdf-engine=xelatex` (and a custom engine from config) with a `….tmp-{pid}.pdf` out path;
  typography=false flips the input format to `markdown-smart` on BOTH sink paths; html/docx
  argv otherwise identical to today's modulo the corrected temp shape (pin the exact vectors).
  Config-fold tests per the existing section patterns ([export] absent → defaults; partial
  section → per-field inherit).
- **B3:** the two NAMED updated tests + the new default-theme shade test, PLUS the geometry
  sweep — run the full suite after the flip and re-point every legitimately-shifted
  nav/caret/layout/render expectation for inactive headings under the flipped themes (each
  re-point verified as the new CORRECT value, never a weakening).
- **A2:** the new row-0 style test (the corrected gap-cells + no-base-bg form from
  Component 3 — fails today, passes with the fill).
- **Manual eyeball pass per theme** (backlog decision #8 requires it): before the merge
  report, view a headed document under default/tokyo_night/a base16/phosphor(±flat)/no_color
  and confirm the glyph ramp reads well in each — recorded as a checklist line in the
  pre-merge report beside the smoke summary.
- Suite green (`cargo test -p wordcartel-core -p wordcartel`), workspace clippy deny-gate
  clean, build/test-compile warning-free. **PTY smoke suite: mandatory-run, advisory-pass**
  — the pre-merge report quotes `scripts/smoke/run.sh`'s one-line summary verbatim (house
  rule, 2026-07-03). No export test spawns pandoc (the argv seam is the test surface).

## Decomposition (3 tasks)

1. **C1 export** — `ExportConfig`/`RawExport`/fold + `Editor.export_cfg` seeding + the
   `pandoc_argv` seam + per-format args (`-s` for tex, `--pdf-engine` for pdf, typography
   format string) + the `export_tex` registry entry + argv/config unit tests.
2. **B3 glyphs** — flip the four constructor defaults (in `phosphor`, `flat` keeps controlling
   face selection (:502-507) + `monochrome`; only its `heading_level_glyph` feed is removed)
   + update the two tests + the new default-theme shade test.
3. **A2 bar fill** — the full-width Chrome fill + the row-0 style test.

## Global constraints

- `cargo test -p wordcartel-core -p wordcartel` green; workspace clippy **deny** gate clean;
  no `cargo fmt`; house style (em-dash `—`); `#![forbid(unsafe_code)]` untouched.
- Smoke suite run + verbatim summary in the pre-merge report (advisory, never a merge gate).
- Behavior changes are exactly the decided ones (glyph defaults INCLUDING their inherent
  two-cell geometry shift for inactive headings, pdf engine default, the bar fill) plus the
  temp-naming CORRECTION if today's docx/pdf inference proves broken (a bug fix, stated in
  the merge report); everything else identical — the argv unit tests pin html/docx against
  the corrected shape.

## Plan-confirms (resolve during the implementation plan, against real source)

1. The exact current argv construction at BOTH sites (Capture `export.rs:131-146`,
   WritesOutput `:160-166`) and the cleanest extraction shape for `pandoc_argv` (+ how the
   opts thread through `do_export`'s closure into `run_pandoc` — extend the signature or an
   `ExportOpts` struct).
2. The `[export]` fold: exact `RawConfig` field + `load()` fold lines to mirror
   (config.rs:288-309), and where `Editor.export_cfg` defaults in `new_from_text`.
3. `phosphor`'s `flat` param after the flip: it keeps controlling the face-branch selection
   (theme.rs:502-507) and `monochrome` (:536); only its `heading_level_glyph` feed is removed.
   Grep `flat` in theme.rs to confirm no additional consumer, and confirm the flat/non-flat
   test pair (`phosphor_flat_is_monochrome_single_shade`,
   `phosphor_shaded_distinguishes_by_shade`) stays green.
4. The exact new expected row-0 string for `renders_concealed_heading_and_cursor_on_active_line`
   after the flip (run the render; likely `"█ Title…"`) — re-point, never weaken. Then the
   BROADER geometry sweep: enumerate every additional failing test after the flip (nav/caret/
   layout/render under default/tokyo_night/base16 with inactive headings), verify each new
   value is geometrically correct (the two-cell prefix), and re-point.
5. RESOLVED (Codex round 1): the fill primitive is `Buffer::set_style` over the row-0 rect
   (Paragraph delegates to it anyway) — the plan mandates it.
6. RESOLVED (Fable5, live pandoc 3.9.0.2 + TeX Live): (a) today's docx/pdf ARE broken —
   HTML fragments under .docx/.pdf names with success status; (b) the extension-preserving
   fix produces real docx / PDF 1.7; (c) `markdown-smart` is the complete strictness story
   incl. the latex writer; (d) `--pdf-engine=xelatex` accepted, bad engines exit 6 →
   `FilterError::NonZero` status; (e) `-s -t latex` compiles standalone under xelatex.
   Unit tests pin only argv SHAPE.
7. RESOLVED (Fable5 grep): nothing cleans/globs the old `*.tmp-{pid}` pattern anywhere in
   src/ or scripts/.
8. The `do_export` second call site (app.rs:699, `resolve_prompt`'s `OverwriteExport` arm):
   confirm the reads-`editor.export_cfg`-itself shape keeps BOTH callers signature-stable
   (rustc backstops a miss regardless).
