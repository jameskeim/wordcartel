# UX Quick-Wins Bundle (A2 + B3 + C1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the first three settled UX-backlog items: C1 — `export_tex` + `[export]` config (xelatex PDF engine, export typography) **which also fixes the confirmed docx/pdf export bug** (today they silently write HTML fragments under .docx/.pdf names); B3 — heading glyphs default ON in every theme; A2 — full-width Chrome fill of the menu-bar row.

**Architecture:** Shell-only (+ 4 one-line theme flips in core's `theme.rs`). C1: a new config section threaded onto `Editor`, read by `do_export` itself (covers both call sites), with a pure `pandoc_argv` seam + extension-preserving temp names. B3: constructor default flips (geometry change owned by the spec). A2: one `set_style` fill before the label loop.

**Tech Stack:** Rust, serde/toml config, pandoc (subprocess; NOT spawned by tests — the argv seam is the test surface).

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-03-wordcartel-ux-quick-wins-design.md` (Codex ×3 + Fable5; the pandoc semantics are EMPIRICALLY RESOLVED there — do not re-litigate them).
- `cargo test -p wordcartel-core -p wordcartel` green; `cargo build`/`test --no-run` warning-free; **`cargo clippy --workspace --all-targets` clean (deny gate LIVE)**; NO `cargo fmt`; house style (em-dash `—`).
- **Never weaken a test to make it pass.** B3's geometry sweep RE-POINTS shifted expectations to verified-correct new values; A2's row test must NOT degrade into an `any()` probe.
- **Pre-merge report requirements:** run `scripts/smoke/run.sh` and quote its one-line summary verbatim (mandatory-run, advisory-pass); record the per-theme eyeball pass; state the confirmed docx/pdf bug fix explicitly.
- Trailers on every commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: C1 — export config + `pandoc_argv` seam + `export_tex` (and the bug fix)

**Files:**
- Modify `wordcartel/src/config.rs` (ExportConfig + RawExport + Config/RawConfig fields + fold).
- Modify `wordcartel/src/editor.rs` (Editor field + `new_from_text` init).
- Modify `wordcartel/src/app.rs` (seed in `run()`).
- Modify `wordcartel/src/export.rs` (ExportOpts, `pandoc_argv`, `temp_path_for`, `run_pandoc`, `do_export`; tests).
- Modify `wordcartel/src/registry.rs` (the `export_tex` entry).

**Interfaces produced:** `config::ExportConfig { pdf_engine: String, typography: bool }`; `Editor.export_cfg`; `export::ExportOpts`; pure `pandoc_argv` + `temp_path_for`; the `export_tex` command.

- [ ] **Step 1: Config section.** In `config.rs`:
  - Resolved struct (beside `ViewConfig`, ~:72):
```rust
#[derive(Debug, Clone)]
pub struct ExportConfig {
    /// Pandoc PDF engine (`--pdf-engine=…`). Default xelatex (deliberate; see the spec).
    pub pdf_engine: String,
    /// Export-time smart punctuation. true → `-f markdown` (pandoc's smart default);
    /// false → `-f markdown-smart` (strict literal). Applies to all export formats.
    pub typography: bool,
}
impl Default for ExportConfig {
    fn default() -> Self {
        ExportConfig { pdf_engine: "xelatex".into(), typography: true }
    }
}
```
  - `Config` (:33-41) gains `pub export: ExportConfig,`.
  - Raw side (beside `RawView`, ~:198):
```rust
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawExport {
    pdf_engine: Option<String>,
    typography: Option<bool>,
}
```
  - `RawConfig` (:139-146) gains `export: RawExport,`.
  - Fold (beside the view folds, ~:288-309 — simple per-field, no validation needed):
```rust
        // export: per-field override (omitted field inherits the lower layer).
        if let Some(v) = raw.export.pdf_engine { cfg.export.pdf_engine = v; }
        if let Some(v) = raw.export.typography { cfg.export.typography = v; }
```

- [ ] **Step 2: Config tests** (in `config.rs`'s test module, mirroring the existing section-fold tests — locate them and match style): (a) no `[export]` section → defaults (`"xelatex"`, `true`); (b) partial section (`[export]\ntypography = false`) → `pdf_engine` stays default, `typography` false; (c) two layers where the higher sets only `pdf_engine = "tectonic"` → typography inherits the lower. Run: `cargo test -p wordcartel config` → PASS.

- [ ] **Step 3: Thread onto Editor.** `editor.rs`: add `pub export_cfg: crate::config::ExportConfig,` to `Editor` and `export_cfg: crate::config::ExportConfig::default(),` in `new_from_text` (beside `diag_cfg`, ~:446). `app.rs` `run()`: `editor.export_cfg = cfg.export.clone();` beside `editor.diag_cfg = …` (~:1959). (The seeding line itself is untested — precedent-consistent with `view_opts`/`diag_cfg`, and no dispatch precedes it; record this accepted gap in the ledger — Fable Minor-2.)

- [ ] **Step 4: The export.rs rework.** Add the opts type + the two pure helpers; rewire:

```rust
/// Resolved per-dispatch export options (read from `Editor.export_cfg` by `do_export`,
/// so BOTH call sites — run_export and the OverwriteExport prompt arm — get them).
pub(crate) struct ExportOpts {
    pub typography: bool,
    pub pdf_engine: String,
}

/// Extension-preserving temp path beside `target`: `{stem}.tmp-{pid}.{ext}`.
/// The extension MUST stay visible to pandoc's `-o` format inference — the old
/// `{name}.tmp-{pid}` shape hid it, making pandoc default to HTML (the confirmed
/// docx/pdf bug; see the spec).
fn temp_path_for(target: &Path, ext: &str, pid: u32) -> PathBuf {
    let stem = target.file_stem().unwrap_or_default().to_string_lossy();
    let tmp_name = format!("{stem}.tmp-{pid}.{ext}");
    target.parent().map(|p| p.join(&tmp_name)).unwrap_or_else(|| PathBuf::from(&tmp_name))
}

/// Compose the WritesOutput invocation: the extension-preserving temp path AND the argv
/// built from THAT SAME path — one pure function, so the composition (not just the two
/// halves) is unit-testable. This is the guard against the exact bug class this effort
/// fixes: a future regression that rebuilds `tmp` differently would break the
/// composition test, not sail through green piece-tests (Fable plan review I-1, adopted).
fn writes_output_invocation(
    target: &Path, ext: &str, pid: u32, opts: &ExportOpts,
) -> (PathBuf, Vec<String>) {
    let tmp = temp_path_for(target, ext, pid);
    let argv = pandoc_argv(
        &ExportSink::WritesOutput { ext: ext.to_owned() },
        Some(&tmp),
        opts,
    );
    (tmp, argv)
}

/// Build the pandoc argv for one export. Pure — the testable seam. `out` is the
/// ALREADY-DERIVED temp path (None for the Capture/html sink; `pandoc_argv` never
/// constructs a path — the spec's contract holds).
fn pandoc_argv(sink: &ExportSink, out: Option<&Path>, opts: &ExportOpts) -> Vec<String> {
    let input = if opts.typography { "markdown" } else { "markdown-smart" };
    let mut argv = vec!["pandoc".to_owned(), "-f".to_owned(), input.to_owned()];
    match sink {
        ExportSink::Capture { ext } => {
            argv.push("-t".to_owned());
            argv.push(ext.clone());
        }
        ExportSink::WritesOutput { ext } => {
            if ext == "tex" {
                // Standalone + explicit format: a compilable document, no inference.
                argv.push("-s".to_owned());
                argv.push("-t".to_owned());
                argv.push("latex".to_owned());
            }
            if ext == "pdf" {
                argv.push(format!("--pdf-engine={}", opts.pdf_engine));
            }
            argv.push("-o".to_owned());
            argv.push(out.expect("WritesOutput requires an out path").to_string_lossy().into_owned());
        }
    }
    argv
}
```
  - `do_export` (:90-112): build `let opts = ExportOpts { typography: editor.export_cfg.typography, pdf_engine: editor.export_cfg.pdf_engine.clone() };` before the spawn; move `opts` into the closure; call `run_pandoc(sink, &stdin, &target, &opts)`.
  - `run_pandoc` (:123-190): new param `opts: &ExportOpts`. **The restructure shape is
    PRESCRIBED (Codex Critical — the current `match sink` MOVES `ext` out at :149, so a
    literal "compute tmp then `pandoc_argv(&sink,…)`" is a use-after-partial-move):**
    change to **`match &sink { … }`** throughout — the WritesOutput arm binds `ext: &String`
    and consumes the COMPOSITION seam:
    `let (tmp, argv) = writes_output_invocation(&target, ext, std::process::id(), opts);`
    then runs the subprocess, checks `tmp.exists()`, returns `TempReady(tmp)` — the arm never
    calls `temp_path_for`/`pandoc_argv` directly, so the tmp fed to `-o` is BY CONSTRUCTION
    the tmp that is checked and renamed. The Capture arm:
    `let argv = pandoc_argv(&sink, None, opts);` (byte-identical to today when
    typography=true — pinned in tests). Delete the `let _ = ext;` hack (ext is now used).
    **DO NOT clone the sink or `ext` to appease the borrow checker** — `match &sink` is the
    compiling form.
  - **Stale docs (Fable Minor-1):** update the module doc (`:3` "Three presets: html, docx,
    pdf" → the four formats), `do_export`'s doc (`:88-89` — it documents the OLD buggy
    `-o <target>.tmp-<pid>` shape), and tidy the muddled stdin comment block (`:167-170`).
  - Everything else (timeout, max_output, `run_subprocess`, `tmp.exists()` check, `guarded_export`, `Msg::ExportDone`, TOCTOU, status strings) unchanged.

- [ ] **Step 5: The `export_tex` command** (`registry.rs`, after `export_pdf` :185-188):
```rust
        r.register("export_tex", "Export LaTeX", Some(MenuCategory::Export), |c| {
            crate::export::run_export(c.editor, "tex", &c.msg_tx);
            CommandResult::Handled
        });
```

- [ ] **Step 6: Argv + temp unit tests** (export.rs tests mod). Pin the EXACT vectors:
```rust
    fn opts(typo: bool, engine: &str) -> ExportOpts {
        ExportOpts { typography: typo, pdf_engine: engine.into() }
    }

    #[test]
    fn argv_html_matches_today_when_typography_on() {
        let a = pandoc_argv(&ExportSink::Capture { ext: "html".into() }, None, &opts(true, "xelatex"));
        assert_eq!(a, vec!["pandoc", "-f", "markdown", "-t", "html"]);
    }
    #[test]
    fn argv_typography_off_uses_markdown_smart_minus() {
        let a = pandoc_argv(&ExportSink::Capture { ext: "html".into() }, None, &opts(false, "xelatex"));
        assert_eq!(a, vec!["pandoc", "-f", "markdown-smart", "-t", "html"]);
    }
    #[test]
    fn argv_docx_gets_extension_preserving_out_path() {
        let out = std::path::Path::new("/a/notes.tmp-123.docx");
        let a = pandoc_argv(&ExportSink::WritesOutput { ext: "docx".into() }, Some(out), &opts(true, "xelatex"));
        assert_eq!(a, vec!["pandoc", "-f", "markdown", "-o", "/a/notes.tmp-123.docx"]);
    }
    #[test]
    fn argv_pdf_carries_the_engine_flag() {
        let out = std::path::Path::new("/a/notes.tmp-123.pdf");
        let a = pandoc_argv(&ExportSink::WritesOutput { ext: "pdf".into() }, Some(out), &opts(true, "tectonic"));
        assert_eq!(a, vec!["pandoc", "-f", "markdown", "--pdf-engine=tectonic", "-o", "/a/notes.tmp-123.pdf"]);
    }
    #[test]
    fn argv_tex_is_standalone_explicit_latex() {
        let out = std::path::Path::new("/a/notes.tmp-123.tex");
        let a = pandoc_argv(&ExportSink::WritesOutput { ext: "tex".into() }, Some(out), &opts(true, "xelatex"));
        assert_eq!(a, vec!["pandoc", "-f", "markdown", "-s", "-t", "latex", "-o", "/a/notes.tmp-123.tex"]);
    }
    #[test]
    fn temp_path_preserves_the_format_extension() {
        let t = temp_path_for(std::path::Path::new("/a/b/notes.pdf"), "pdf", 123);
        assert_eq!(t, std::path::Path::new("/a/b/notes.tmp-123.pdf"));
    }
    #[test]
    fn writes_output_invocation_composes_tmp_and_argv_coherently() {
        // The composition guard (Fable I-1): the argv's -o element IS the returned tmp,
        // and the tmp carries the format extension — a regression that rebuilds either
        // half differently fails HERE even if the piece-tests stay green.
        let (tmp, argv) =
            writes_output_invocation(std::path::Path::new("/a/notes.pdf"), "pdf", 123, &opts(true, "xelatex"));
        let o_pos = argv.iter().position(|a| a == "-o").expect("-o present");
        assert_eq!(argv[o_pos + 1], tmp.to_string_lossy(), "argv -o must be the returned tmp");
        assert!(tmp.extension().is_some_and(|e| e == "pdf"), "tmp must end with the format ext: {tmp:?}");
    }
```

- [ ] **Step 7: Run + gates + commit.** `cargo test -p wordcartel` green; clippy clean.
```bash
git add -A
git commit -m "feat(export): export_tex + [export] config (xelatex, typography) + fix silent docx/pdf HTML-fragment bug"   # + trailers
```

---

### Task 2: B3 — heading glyphs default ON in every theme

**Files:** Modify `wordcartel-core/src/theme.rs` (4 flips + 1 test); `wordcartel/src/render.rs` (1 test re-point + 1 new test); any additional tests the sweep surfaces.

- [ ] **Step 1: The four flips** (`theme.rs`):
  - `:232` (`default()`): `heading_level_glyph: false,` → `heading_level_glyph: true,` (leave `monochrome: false`).
  - `:288` (`tokyo_night()`): `false` → `true`.
  - `:349` (`from_base16()`): `false` → `true`.
  - `:536` (`phosphor(...)`): `heading_level_glyph: flat` → `heading_level_glyph: true` (KEEP `monochrome: flat` — `flat` also still selects the face branch at :502-507).

- [ ] **Step 2: Re-point the two named tests.**
  - `theme.rs:615` (`default_base_is_terminal_default`): `assert!(!t.heading_level_glyph);` → `assert!(t.heading_level_glyph);` (update the test's intent comment if it mentions the old default).
  - `render.rs:1138-1148` (`renders_concealed_heading_and_cursor_on_active_line`): the inactive H1 row now renders the shade prefix. Update the doc comment (`/// Row 0 … must show "█ Title" (shade prefix + concealed "# ").`) and the assertion:
```rust
        assert!(row0.starts_with("█ Title"), "expected '█ Title...' got {:?}", row0);
```
    (Plan-confirm: run it — if the actual prefix differs, e.g. spacing, re-point to the REAL rendered string after verifying it's the correct glyph form; never loosen to `contains("Title")`.)

- [ ] **Step 3: The new default-theme pin** (render.rs tests):
```rust
    /// The flipped default: colored themes now render the heading shade ramp (B3).
    #[test]
    fn default_theme_renders_heading_shade_prefix() {
        let mut e = Editor::new_from_text("# One\n\n## Two\n\nbody\n", None, (20, 8));
        set_caret(&mut e, 15); // byte 15 = the 'b' of "body" (Codex-verified) — both headings inactive
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(20, 8)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();
        let row = |y: u16| -> String { (0u16..20).map(|x| buf[(x, y)].symbol().chars().next().unwrap_or(' ')).collect() };
        assert!(row(0).starts_with("█ One"), "H1 shade: got {:?}", row(0));
        assert!(row(2).starts_with("▓ Two"), "H2 shade: got {:?}", row(2));
    }
```
    (Rows Codex-verified: H1 row 0, blank row 1, H2 row 2, blank row 3, body row 4 — the
    assertions above are correct as written.)

- [ ] **Step 4: The bounded sweep.** Run `cargo test -p wordcartel-core -p wordcartel`. Per the spec's bound: `display`-string assertions survive (the glyph feeds `prefix_glyph`, not `display`); expect at most a handful of exact-row-string/column failures under default/tokyo_night/base16 with INACTIVE headings. **Codex pre-warn list:** definite = render.rs:1147 (Step 2 handles it); monitor-but-likely-green = `nav.rs:955-969` (`heading_glyph_layout_geometry_under_no_color` — no_color, flag already on), `nav.rs:1419-1428` (`offset_at_cell_never_returns_a_hidden_line` — width 80, the 2-cell prefix shouldn't wrap), `derive.rs:398` + `commands.rs:1191/:1201` (display-based, survive). For EACH failure: verify the new value is exactly the 2-cell prefix shift (or the shade string), then re-point. Record every re-pointed test in the task report. If a failure is NOT explainable by the 2-cell prefix/shade, STOP and report it (that would be a real regression, not the sweep).

- [ ] **Step 5: Run + gates + commit.** Suite green; clippy clean; `cargo test --no-run` warning-free.
```bash
git add -A
git commit -m "feat(theme): heading level glyphs default ON in every theme (B3)"   # + trailers
```

---

### Task 3: A2 — menu bar full-width fill

**Files:** Modify `wordcartel/src/render.rs` (the fill + 1 new test).

- [ ] **Step 1: The fill.** In the menu block (render.rs:906-940), immediately BEFORE `let bar = menu_bar_layout(...)`:
```rust
            // Full-width bar background: gaps between labels + the right side carry the
            // Chrome style; the per-label paints below overwrite their own rects (A2).
            let bar_row = Rect::new(area.x, area.y, w, 1);
            frame.buffer_mut().set_style(bar_row, menu_closed_style);
```
    (`Frame::buffer_mut()` exists in ratatui 0.30 and its borrow ends at the statement — no
    conflict with the later `render_widget` calls (Codex + Fable verified). **`set_style` is
    the ONLY form** — the spec mandates it; do not substitute a Paragraph (Fable Minor-3
    removed the dead fallback).)

- [ ] **Step 2: The row test** (render.rs tests). The corrected, non-vacuous form (the open label is ALWAYS `ChromeSelected` — asserting all-Chrome is impossible):
```rust
    /// A2: with the menu open, row 0 is a solid bar — every cell styled Chrome (gaps +
    /// right side) or ChromeSelected (the open label); no cell keeps the base background.
    #[test]
    fn menu_bar_row_is_filled_full_width() {
        let mut e = Editor::new_from_text("body\n", None, (40, 8));
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        e.menu = Some(crate::menu::build(&reg, &km));
        derive::rebuild(&mut e);
        let mut term = Terminal::new(TestBackend::new(40, 8)).unwrap();
        term.draw(|f| render(f, &mut e)).unwrap();
        let buf = term.backend().buffer();
        let chrome = compose::compose(&e.theme, e.depth, &[SE::Chrome]).bg;
        let selected = compose::compose(&e.theme, e.depth, &[SE::ChromeSelected]).bg;
        for x in 0u16..40 {
            let bg = buf[(x, 0u16)].style().bg;
            assert!(bg == chrome || bg == selected,
                "row-0 cell {x} not bar-styled: {bg:?} (chrome={chrome:?}, selected={selected:?})");
        }
        // And the RIGHT EDGE specifically is Chrome (it is unpainted today — this fails pre-fix).
        assert_eq!(buf[(39u16, 0u16)].style().bg, chrome, "right edge must carry the Chrome fill");
    }
```
    (Plan-confirm: `menu::build`'s exact name/signature + visibility from render's test module (the hydrate path calls it — mirror that); the `compose`/`SE`/`e.depth` imports as used elsewhere in render tests; that default-theme Chrome bg (Black) ≠ base bg (Default) so the test genuinely fails without the fill. Adjust IMPORT/CONSTRUCTION plumbing as needed — the two assertions are the contract.)

- [ ] **Step 3: Run + gates + commit.** Suite green; clippy clean.
```bash
git add -A
git commit -m "feat(render): full-width Chrome fill for the menu bar row (A2)"   # + trailers
```

---

## Pre-merge checklist (beyond the standard gates)

1. `scripts/smoke/run.sh` — quote the one-line summary verbatim in the report (advisory).
2. **Per-theme eyeball pass** (backlog decision #8): open a headed document under default,
   tokyo_night, a base16 theme, phosphor-green (flat + non-flat), and no_color; confirm the
   ramp reads well. Record one line per theme in the report. (`tui-interact`/tmux drive is
   fine for this.)
3. The merge report states the confirmed docx/pdf bug fix (HTML fragments under
   .docx/.pdf names with success status → now real files, empirically verified in the spec).

## Self-Review

**Spec coverage:** C1 — ExportConfig/RawExport/fold + Editor threading + `do_export`-reads-itself (both call sites incl. app.rs:696-699) + `pandoc_argv` + extension-preserving `temp_path_for` (the bug fix) + the `writes_output_invocation` COMPOSITION seam + test (Fable I-1, user decision A — honors the spec's "pandoc_argv never constructs a path" contract) + `-s -t latex` + `--pdf-engine=` + typography format string + `export_tex` registry entry + exact-vector tests + stale-doc updates ✓. B3 — four flips (phosphor keeps `flat`→faces+monochrome) + two re-points + the new default pin + the bounded sweep with stop-on-unexplained rule ✓. A2 — set_style fill (Paragraph fallback) + the corrected non-vacuous row test ✓. Smoke + eyeball + bug-fix statement in the checklist ✓.

**Placeholder scan:** none — every code step is complete; the flagged plan-confirms (exact rendered strings, row indices, `menu::build` plumbing, `buffer_mut` borrow form) each carry re-point-not-weaken rules.

**Type consistency:** `ExportOpts { typography: bool, pdf_engine: String }` matches `ExportConfig`; `pandoc_argv(&ExportSink, Option<&Path>, &ExportOpts) -> Vec<String>`; `temp_path_for(&Path, &str, u32) -> PathBuf`; `run_pandoc` gains `&ExportOpts` + the PRESCRIBED `match &sink` shape (no clones); `assert_eq!(Vec<String>, vec![&str…])` COMPILES as written (Codex: `String: PartialEq<&str>` blanket impl — keep the readable form; tidy only if clippy objects). `ExportConfig: Default` is REQUIRED (Config derives Default at :33 — Codex-confirmed no manual `Config{…}` construction sites break).

**Ordering:** the three tasks are independent (different files except render.rs tests, which T2 and T3 both touch — sequential tasks avoid conflicts); T1 first (largest), then T2 (its sweep wants a quiet baseline), then T3.
