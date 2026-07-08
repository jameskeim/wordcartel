# Wordcartel polish + hygiene batch — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement
> this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship six small, independent polish/hygiene changes as one reviewed effort.

**Architecture:** Six self-contained tasks, each touching one subsystem with its own tests; no shared
code. Executed on branch `effort-polish-batch`, one implementer + one two-verdict reviewer per task,
ordered trivial→design (E5 last/separable). Spec: `docs/superpowers/specs/2026-07-08-wordcartel-polish-batch-design.md`.

**Tech Stack:** Rust (workspace: pure `wordcartel-core` + shell `wordcartel`), Arch PKGBUILD.

## Global Constraints

- `cargo test` green across all suites; `cargo build` + `cargo test --no-run` warning-free for touched
  crates; **`cargo clippy --workspace --all-targets` clean is a GATE** (new warnings fail).
- **Do NOT run `cargo fmt`.** Hand-match the dense house style: 4-space indent, ~100-col hand-wrapped,
  em-dash `—` in prose comments (never `--`), no emoji in code, imports grouped by hand.
- `#![forbid(unsafe_code)]` holds. No new `.unwrap()` on fallible/external paths; typed errors to the
  status line.
- Every commit ends with the trailers verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01JE9im4BoxLa1NeARq1nkyj
  ```
- Command-surface contract: only Task 4 (A7) touches the menu surface and CONFORMS (rendering-only,
  state-in-label preserved); all others are N/A.

---

### Task 1: H4 — Arch `optdepends` for pandoc + xelatex

**Files:**
- Modify: `packaging/arch/PKGBUILD` (optdepends array, lines 21-27)
- Regenerate: `packaging/arch/.SRCINFO`

**Interfaces:** none (packaging metadata only).

- [ ] **Step 1: Add the two optdepends entries.** In `packaging/arch/PKGBUILD`, the array is:

```sh
optdepends=(
  'wayland: native Wayland clipboard (dlopened at runtime)'
  'libxcb: native X11 clipboard (dlopened at runtime)'
  'libx11: native X11 clipboard (dlopened at runtime)'
  'wl-clipboard: wl-copy external clipboard provider (fallback chain)'
  'xclip: xclip external clipboard provider (fallback chain)'
)
```

Append two lines before the closing `)`:

```sh
  'pandoc-cli: markdown export to html/docx/pdf (shells out to pandoc)'
  'texlive-xetex: PDF export via pandoc --pdf-engine=xelatex'
```

Do NOT touch `pkgver`, `pkgrel`, or the `pkgver()` function (VCS-style package; `pkgrel` stays `1`).

- [ ] **Step 2: Regenerate `.SRCINFO`.**

Run: `cd packaging/arch && makepkg --printsrcinfo > .SRCINFO`
Expected: exits 0; the new `.SRCINFO` contains `optdepends = pandoc-cli: ...` and
`optdepends = texlive-xetex: ...`, and its `pkgver` reflects `git describe`.

- [ ] **Step 3: Verify the PKGBUILD parses.**

Run: `cd packaging/arch && makepkg --printsrcinfo | grep -E 'pandoc-cli|texlive-xetex'`
Expected: both lines present. (If `makepkg` is unavailable on the machine, `bash -n PKGBUILD` for syntax
and a manual diff of `.SRCINFO` is the fallback — note it in the report.)

- [ ] **Step 4: Commit.**

```bash
git add packaging/arch/PKGBUILD packaging/arch/.SRCINFO
git commit  # message: "packaging(arch): declare pandoc-cli + texlive-xetex optdepends (H4)"
```

---

### Task 2: ux-H2 — end-of-buffer active-line clamp

**Files:**
- Modify: `wordcartel/src/derive.rs:279-283`
- Test: `wordcartel/src/derive.rs` (`#[cfg(test)]`, near `active_line_renders_raw` at `:503`)

**Interfaces:**
- Consumes: `TextBuffer::len()` (`buffer.rs:18`, = `rope.len_bytes()`), `TextBuffer::byte_to_line()`
  (`buffer.rs:78`), `TextBuffer::is_empty()` (`buffer.rs:22`). `b.document.selection.primary().head`
  is the caret byte.
- Produces: unchanged `active_line: usize` — stored as the `LayoutKey { active_line }` field
  (`derive.rs:306`) and used only in an equality compare (`derive.rs:333`), never as a slice/index.

- [ ] **Step 1: Write the failing test.** Add to the `derive.rs` tests module:

```rust
/// ux-H2: with the caret on the phantom line past a trailing newline, the last CONTENT
/// line must conceal (show "Title"), not stay active and render raw ("# Title").
#[test]
fn caret_on_phantom_line_conceals_last_content_line() {
    let mut e = Editor::new_from_text("# Title\n", None, (80, 24));
    // Caret at buf.len() = 8 — the phantom line past the trailing '\n', NOT on line 0.
    e.active_mut().document.selection = wordcartel_core::selection::Selection::single(8);
    rebuild(&mut e);
    let (rows0, _) = &e.active().view.line_layouts[&0];
    assert_eq!(rows0[0].display, "Title", "last content line must conceal when caret is past the newline");
}
```

- [ ] **Step 2: Run it — verify it FAILS.**

Run: `cargo test -p wordcartel --lib derive::tests::caret_on_phantom_line_conceals_last_content_line`
Expected: FAIL — `assertion failed: left == right`, left `"# Title"` (raw, the bug), right `"Title"`.

- [ ] **Step 3: Apply the fix.** Replace `wordcartel/src/derive.rs:279-283`:

```rust
        let active_line = if buf.is_empty() {
            0
        } else {
            buf.byte_to_line(caret_byte.min(buf.len().saturating_sub(1)))
        };
```

with (drop the `-1`; clamp to `len`, which is always a valid boundary):

```rust
        let active_line = if buf.is_empty() {
            0
        } else {
            // Clamp to `len`, NOT `len-1`: a caret on the phantom line past a trailing newline
            // must map to the phantom line so the last CONTENT line conceals like any inactive
            // line (ux-H2), instead of staying "active" and rendering raw. `len` is a boundary.
            buf.byte_to_line(caret_byte.min(buf.len()))
        };
```

- [ ] **Step 4: Run the new test + the neighbors — verify PASS.**

Run: `cargo test -p wordcartel --lib derive::`
Expected: PASS, including `active_line_renders_raw` (caret at 0 → raw, unchanged) and
`derive_lays_out_visible_lines_with_roles` (caret mid-buffer → concealed, unchanged).

- [ ] **Step 5: Confirm no other `active_line` consumer indexes with it.**

Run: `grep -n active_line wordcartel/src/derive.rs`
Expected: only the `LayoutKey { active_line }` field (`:306`) and the `l == active_line` equality
(`:333`); no slicing/indexing. Note the result in the report.

- [ ] **Step 6: Commit.** `git commit` — "fix(derive): caret past trailing newline conceals last line (ux-H2)"

---

### Task 3: B5 — heading gutter glyph ramp

**Files:**
- Modify: `wordcartel/src/render.rs:18` (the `SHADES` const)
- Test/doc: `wordcartel/src/render.rs` (`:1161`, `:2344`, `:2351-2356`, `:2370-2375`)

**Interfaces:** `SHADES` is indexed at `render.rs:665` and `:730` via `SHADES[(n.clamp(1,6)-1)]` — the
indexing is unchanged; only the glyph table changes.

- [ ] **Step 1: Update the failing golden expectations first (TDD for a table change).** These assertions
  encode the OLD glyphs and will fail once the const changes; update them to the new ramp so they
  express the intended behavior:
  - `render.rs:1161` — change the expected gutter from `"▓ Two"` to `"▆ Two"` (default-theme H2).
  - `render.rs:2344` — change `text.contains('▒')` (H3) to `text.contains('▅')`.
  - `render.rs:2351-2356` — update the doc-comment level→glyph map to
    `H1=█ H2=▆ H3=▅ H4=▄ H5=▃ H6=▂`.
  - `render.rs:2370-2375` — update the six no-color golden assertions to `█ ▆ ▅ ▄ ▃ ▂` respectively
    (and their message strings).

- [ ] **Step 2: Confirm the migration set is complete.**

Run: `grep -nE '█|▓|▒|░|▏|·' wordcartel/src/render.rs`
The grep also matches UNRELATED hits that must NOT change: a `·` separator in status/word-count text
(`:357, :382, :917`) and H1-only `█` assertions (`:1136` — H1 stays `█` in the new ramp). The
heading-shade migration set is exactly `:18` (the const), `:1161` (H2 `▓`→`▆`), `:2344` (H3 `▒`→`▅`),
`:2351-2356` (doc map), `:2370-2375` (no-color goldens). Classify each grep hit: heading-shade gutter
→ migrate; separator/status `·` or H1-only `█` → leave. If a NEW heading-shade site appears, update it
and note it.

- [ ] **Step 3: Change the const.** At `wordcartel/src/render.rs:18`:

```rust
const SHADES: [&str; 6] = ["█", "▓", "▒", "░", "▏", "·"];
```
→
```rust
// Heading gutter ramp: a single-axis lower-block height ramp (decreasing solid mass), collision-free
// with the blockquote bar (▎) and list bullet (•). U+2588 2586 2585 2584 2583 2582.
const SHADES: [&str; 6] = ["█", "▆", "▅", "▄", "▃", "▂"];
```

- [ ] **Step 4: Run the render tests — verify PASS.**

Run: `cargo test -p wordcartel --lib render::`
Expected: PASS. If a golden shows a column shift (glyph width), investigate — but `▆▅▄▃▂` are
single-width cells, so none is expected.

- [ ] **Step 5: Commit.** `git commit` — "feat(render): single-axis heading glyph ramp █▆▅▄▃▂ (B5)"

---

### Task 4: A7 — right-justified value column in stateful menu rows

**Files:**
- Modify: `wordcartel/src/menu.rs` (`grouped_commands` `:31`, `menu_leaf_base` `:59`,
  `right_justify_leaves` `:77`)
- Test: `wordcartel/src/menu.rs` (update `:158`, `:174`; add one)

**Interfaces:**
- Consumes: `MenuMark { OnOff(bool), Value(&'static str) }` (`registry.rs:47`); `keymap.chord_for(id)`.
- Produces: `right_justify_leaves` new signature
  `Vec<(String, Option<String>, Option<String>, CommandId)> -> Vec<(String, CommandId)>` (adds the value
  as the 2nd tuple slot). `grouped_commands` output unchanged (`Vec<(MenuCategory, Vec<(String, CommandId)>)>`).

- [ ] **Step 1: Write the failing tests.**

Update `menu_leaf_shows_state_in_label` (`:158`) — the value is no longer glued after `: `:

```rust
    #[test]
    fn menu_leaf_shows_state_in_label() {
        let reg = crate::registry::Registry::builtins();
        let (km, _) = crate::keymap::build_keymap(&crate::config::KeymapConfig::default(), &reg);
        let mut ed = crate::editor::Editor::new_from_text("x\n", None, (40, 8));
        ed.view_opts.word_count = true;
        let groups = grouped_commands(&reg, &km, &ed);
        let view = groups.iter().find(|(c, _)| *c == crate::registry::MenuCategory::View).unwrap();
        // A7: base name and value shown, value in its own column — the glued "Word Count: On" is gone.
        assert!(view.1.iter().any(|(label, _)|
            label.starts_with("Word Count") && label.contains("On") && !label.contains("Word Count: On")),
            "stateful toggle shows 'Word Count' + 'On' in a column, got {:?}", view.1);
    }
```

Update `menu_chords_are_right_justified_within_a_group` (`:174`) to the new 4-tuple signature and add a
value-column assertion:

```rust
    #[test]
    fn menu_chords_are_right_justified_within_a_group() {
        // 4-tuple: (base, value, chord, id). Chords still flush-right; values (when present) share a column.
        let raw = vec![
            ("Cut".to_string(), None, Some("ctrl-x".to_string()), CommandId("cut")),
            ("Copy As Something Long".to_string(), None, Some("ctrl-c".to_string()), CommandId("copy")),
            ("No Chord Item".to_string(), None, None, CommandId("noop")),
        ];
        let leaves = right_justify_leaves(raw);
        assert!(leaves[0].0.ends_with("ctrl-x"));
        assert!(leaves[1].0.ends_with("ctrl-c"));
        assert_eq!(leaves[0].0.chars().count(), leaves[1].0.chars().count());
        assert_eq!(leaves[2].0, "No Chord Item", "no value + no chord → bare base, no trailing pad");
    }

    #[test]
    fn menu_values_share_a_right_aligned_column() {
        // Differing base widths, values of differing widths → all value right-edges align. One row
        // carries BOTH a value and a chord (the user-customized-binding case).
        let raw = vec![
            ("Clipboard".to_string(), Some("Auto".to_string()), None, CommandId("a")),
            ("Keymap".to_string(), Some("CUA".to_string()), Some("ctrl-k".to_string()), CommandId("b")),
            ("Word Count".to_string(), Some("Off".to_string()), None, CommandId("c")),
            ("Plain Item".to_string(), None, None, CommandId("d")),
        ];
        let leaves = right_justify_leaves(raw);
        // The value-column right edge is common: the char index just past each value aligns.
        let val_end = |s: &str, v: &str| s.find(v).map(|i| i + v.chars().count());
        let e_clip = val_end(&leaves[0].0, "Auto").unwrap();
        let e_wc   = val_end(&leaves[2].0, "Off").unwrap();
        assert_eq!(e_clip, e_wc, "value right edges must align: {:?}", leaves);
        // The both-columns row keeps the chord flush-right after the value column.
        assert!(leaves[1].0.contains("CUA") && leaves[1].0.ends_with("ctrl-k"));
        // A plain row with neither is bare.
        assert_eq!(leaves[3].0, "Plain Item");
    }
```

- [ ] **Step 2: Run — verify FAIL.**

Run: `cargo test -p wordcartel --lib menu::`
Expected: compile error (old `right_justify_leaves` signature) or assertion failures — the value column
does not exist yet.

- [ ] **Step 3: Refactor `menu_leaf_base` → `menu_leaf_parts` (base + value).** Replace `:59-72`:

```rust
/// Base menu leaf text and optional state value, split. Stateless → `(label, None)`.
/// Stateful → `(base, Some(value))` where `base` strips a leading "Toggle " and any ": variants".
fn menu_leaf_parts(meta: &crate::registry::CommandMeta, editor: &crate::editor::Editor)
    -> (String, Option<String>)
{
    use crate::registry::MenuMark;
    match meta.state {
        None => (meta.label.to_string(), None),
        Some(f) => {
            let base = meta.label.strip_prefix("Toggle ").unwrap_or(meta.label);
            let base = base.split(':').next().unwrap_or(base).trim().to_string();
            let value = match f(editor) {
                MenuMark::OnOff(b) => if b { "On" } else { "Off" }.to_string(),
                MenuMark::Value(v) => v.to_string(),
            };
            (base, Some(value))
        }
    }
}
```

- [ ] **Step 4: Thread value through `grouped_commands`.** In `:36-45`, the closure builds the raw
  intermediate. Change it from `(menu_leaf_base(meta, editor), keymap.chord_for(id), id)` to carry the
  value:

```rust
        let mut raw: Vec<(String, Option<String>, Option<String>, CommandId)> = reg
            .commands()
            .filter_map(|(id, meta)| {
                if meta.menu == Some(cat) && id != CommandId("palette") {
                    let (base, value) = menu_leaf_parts(meta, editor);
                    Some((base, value, keymap.chord_for(id), id))
                } else {
                    None
                }
            })
            .collect();
```

And the palette special-case push at `:47`:

```rust
            raw.push(("Command Palette...".to_string(), None, keymap.chord_for(CommandId("palette")), CommandId("palette")));
```

- [ ] **Step 5: Generalize `right_justify_leaves` to two columns.** Replace `:77-94`:

```rust
/// Lay out a dropdown group into two independent right-aligned columns: an optional VALUE column
/// (stateful rows) and the CHORD column, matching the palette. `[base] … [value] … [chord]`.
/// A group with no stateful rows renders byte-identically to the chord-only layout. `GAP` is the
/// min gap before the chord; `VGAP` the gap between the base name and the value column.
fn right_justify_leaves(raw: Vec<(String, Option<String>, Option<String>, CommandId)>)
    -> Vec<(String, CommandId)>
{
    const GAP: usize = 4;
    const VGAP: usize = 2;
    let cc = |s: &str| s.chars().count();
    let max_base = raw.iter().map(|(b, _, _, _)| cc(b)).max().unwrap_or(0);
    let max_val = raw.iter().filter_map(|(_, v, _, _)| v.as_deref().map(cc)).max().unwrap_or(0);
    let has_values = max_val > 0;
    // Left block: base name + (optional) right-aligned value column.
    let left_of = |base: &str, value: &Option<String>| -> String {
        if !has_values {
            base.to_string()
        } else {
            match value {
                Some(v) => format!("{:<mb$}{}{:>mv$}", base, " ".repeat(VGAP), v, mb = max_base, mv = max_val),
                None => format!("{:<w$}", base, w = max_base + VGAP + max_val),
            }
        }
    };
    // Chord column: right-justify to the widest (left-block + GAP + chord) over the group.
    let target = raw.iter()
        .map(|(b, v, c, _)| cc(&left_of(b, v)) + c.as_ref().map_or(0, |c| GAP + cc(c)))
        .max().unwrap_or(0);
    raw.into_iter()
        .map(|(base, value, chord, id)| {
            let label = match (&value, &chord) {
                (_, Some(c)) => {
                    let left = left_of(&base, &value);
                    let pad = target.saturating_sub(cc(&left) + cc(c));
                    format!("{left}{}{c}", " ".repeat(pad))
                }
                (Some(_), None) => left_of(&base, &value), // value column is last — no trailing pad
                (None, None) => base,                      // bare
            };
            (label, id)
        })
        .collect()
}
```

- [ ] **Step 6: Run — verify PASS.**

Run: `cargo test -p wordcartel --lib menu::`
Expected: PASS, including `custom_bind_surfaces_in_menu_and_palette` (`:190` — chord still baked into the
label via `chord_for` at `keymap.rs:194`, user binding preferred at `:199`; `label.contains("ctrl-alt-c")`
still holds).

- [ ] **Step 7: Clippy the crate.**

Run: `cargo clippy -p wordcartel --all-targets`
Expected: clean (watch for `format!` width-arg lints; the named-arg form above is idiomatic).

- [ ] **Step 8: Commit.** `git commit` — "feat(menu): right-align stateful value in its own column (A7)"

---

### Task 5: M5-closeout — cap the dictionary read + correct the stale note

**Files:**
- Modify: `wordcartel/src/app.rs:1447-1451`
- Modify: `CLAUDE.md` ("Remaining before Effort P" paragraph, item (2))
- Test: `wordcartel/src/app.rs` or `file.rs` (dictionary-cap assertion)

**Interfaces:**
- Consumes: `file::bounded_read_opt(path: &Path, limit: u64) -> Option<Vec<u8>>` (`file.rs:133`);
  `crate::limits::MAX_OPEN_BYTES` (`limits.rs:5`, 64 MiB).

- [ ] **Step 1: Write the failing test.** The current load path (`app.rs:1449`) reads unbounded. Add a
  focused test at the `bounded_read_opt` seam that pins the degradation the dict path now relies on
  (an over-cap file yields `None` → the dict stays empty; an in-cap file round-trips). Place it in
  `file.rs` tests near `bounded_read_opt_caps_allocation` (`:227`):

```rust
    #[test]
    fn dictionary_style_read_is_bounded_and_utf8_guarded() {
        // The personal-dictionary load (app.rs) now routes through bounded_read_opt + from_utf8.
        let p = scratch_path("dict-cap");
        std::fs::write(&p, "alpha\nbeta\n").unwrap();
        // In-cap valid file → Some(bytes) → valid UTF-8 → words parse.
        let text = bounded_read_opt(&p, crate::limits::MAX_OPEN_BYTES)
            .and_then(|b| String::from_utf8(b).ok());
        assert_eq!(text.as_deref(), Some("alpha\nbeta\n"));
        // Over-cap file → None → empty dictionary (no slurp, no panic).
        std::fs::write(&p, "x".repeat(10)).unwrap();
        assert_eq!(bounded_read_opt(&p, 4), None, "over-cap → None → empty dict degradation");
    }
```

- [ ] **Step 2: Run — verify PASS-or-FAIL as appropriate.** (This test exercises the existing
  `bounded_read_opt`, so it passes immediately; it documents/guards the seam the dict path adopts. Run it
  to confirm the seam behaves as the fix assumes.)

Run: `cargo test -p wordcartel --lib file::tests::dictionary_style_read_is_bounded_and_utf8_guarded`
Expected: PASS.

- [ ] **Step 3: Cap the dictionary read.** Replace `wordcartel/src/app.rs:1447-1451`:

```rust
    // Load the personal dictionary from disk (missing/unreadable → empty; no abort).
    if let Some(dict_path) = &cfg.diagnostics.dictionary {
        if let Ok(text) = std::fs::read_to_string(dict_path) {
            editor.dictionary = text.lines().map(|l| l.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
    }
```

with:

```rust
    // Load the personal dictionary from disk (missing/unreadable/over-cap/invalid-UTF-8 → empty; no abort).
    if let Some(dict_path) = &cfg.diagnostics.dictionary {
        if let Some(text) = crate::file::bounded_read_opt(dict_path, crate::limits::MAX_OPEN_BYTES)
            .and_then(|bytes| String::from_utf8(bytes).ok())
        {
            editor.dictionary = text.lines().map(|l| l.trim().to_string()).filter(|s| !s.is_empty()).collect();
        }
    }
```

- [ ] **Step 4: Build + test the crate.**

Run: `cargo test -p wordcartel --lib`
Expected: PASS (behavior is identical for all in-cap valid dictionaries).

- [ ] **Step 5: Correct the stale `CLAUDE.md` note.** In the "Hardening campaign" section, item `(2)`
  of "Remaining before Effort P", the current text claims the recovery content-hash / fingerprint /
  save skip-unchanged `fs::read` load paths are unbounded. Replace that clause with the accurate status:

  > (2) M5 follow-ups — finish the undo louder-hint for buffer-level merges. (The document-sized
  > `fs::read` load paths flagged earlier — recovery content-hash, fingerprint, save skip-unchanged, and
  > the swap read — are already capped via `bounded_read_opt`/`read_swap_capped`; the personal-dictionary
  > read is now capped too, so **no document-class unbounded `fs::read` remains**. Small config/theme
  > reads — `config.rs`, startup overrides/mask in `app.rs`, `theme_resolve.rs` — are deliberately
  > unbounded config-class files, out of scope.)

  First READ the exact current wording of item (2) and preserve any sub-point still accurate (the undo
  louder-hint). Match the file's prose style (em-dashes, no `--`).

- [ ] **Step 6: Commit.** `git add wordcartel/src/app.rs wordcartel/src/file.rs CLAUDE.md && git commit`
  — "fix(app): cap personal-dictionary read; correct stale M5 note (M5-closeout)"

---

### Task 6: E5 — chrome bar/status foreground recede

**Files:**
- Modify: `wordcartel-core/src/theme.rs` — `derive_chrome` (`:298-301`), a new const (near `:383`),
  the non-RGB constructors (`:478` terminal-plain, `:611` terminal-ansi, `:1000` `mono_faces` for
  no-color).
- Test: `wordcartel-core/src/theme.rs` — new all-themes ladder test; re-pin the `c_fg` column in the
  pin table (`:1655-1721`) and the standalone pins (`:1854, :1917, :1930, :1941, :2003, :2056`).

**Interfaces:**
- Consumes: `blend(Color, (u8,u8,u8), f32) -> Color` (used at `:307`); `derive_fg` closure (`:286`, floor
  guard); `Face { fg, bg, dim, .. }` (the `dim` field exists, `:25`); `contrast_ratio(Color,Color) -> f32`
  (`:420`); `FG_FLOOR = 4.5` (`:383`).
- Produces: a receded, DIM `Chrome` face fg; NO change to any other face or any bg.

- [ ] **Step 1: Write the failing acceptance test (TDD).** Add to the `theme.rs` tests module. It
  iterates all RGB builtins and asserts the intensity ladder + the dim flag:

```rust
    #[test]
    fn e5_chrome_bar_fg_recedes_below_body_on_every_rgb_theme() {
        use SemanticElement::*;
        // Iterate EVERY builtin (avoids constructor-name drift + auto-covers all phosphor variants and
        // the blue-jeans family). Skip non-RGB bases (terminal-plain/ansi/no-color have Color::Default,
        // so derive_chrome no-ops on them). Bar (Chrome) fg must recede below what body text would be on
        // the same panel, sit above the dropdown, stay ≥ floor, and carry DIM.
        for name in Theme::builtin_names() {
            let mut t = Theme::builtin(name).expect("builtin name resolves");
            let base_fg = t.base_fg;
            if !matches!(base_fg, Color::Rgb { .. }) { continue; } // non-RGB → derive_chrome skips
            t.derive_chrome(ChromeDisposition::Full);
            let chrome = t.face(Chrome);
            let muted = t.face(ChromeMuted);
            let cbg = chrome.bg.expect("derived chrome bg");
            let cfg = chrome.fg.expect("derived chrome fg");
            let cr = |a: Color, b: Color| contrast_ratio(a, b);
            assert!(cr(cfg, cbg) < cr(base_fg, cbg), "{name}: chrome fg must recede below body fg on the bar panel");
            assert!(cr(cfg, cbg) > cr(muted.fg.unwrap(), muted.bg.unwrap()), "{name}: chrome fg must sit above the dropdown");
            assert!(cr(cfg, cbg) >= FG_FLOOR - CR_TOL, "{name}: chrome fg must clear the floor");
            assert_eq!(chrome.dim, Some(true), "{name}: chrome must carry DIM");
        }
    }

    #[test]
    fn e5_non_rgb_chrome_carries_dim() {
        use SemanticElement::*;
        // default() = terminal-plain; terminal_ansi(); no_color() are the three non-RGB builtins.
        for t in [default(), terminal_ansi(), no_color()] {
            assert_eq!(t.face(Chrome).dim, Some(true), "{} chrome must carry DIM", t.name);
        }
    }
```

  This uses `Theme::builtin_names()` (`theme.rs:187`, `&'static [&'static str]`) + `Theme::builtin(name)`
  (`theme.rs:160`, `Option<Theme>`) so it needs NO hand-maintained constructor list. `CR_TOL` is the
  existing tolerance const used by `derive_fg` (`:288/:293`). `default()`/`terminal_ansi()`/`no_color()`
  are real zero-arg constructors.

- [ ] **Step 2: Run — verify FAIL.**

Run: `cargo test -p wordcartel-core --lib theme::tests::e5_`
Expected: FAIL — chrome fg currently equals body fg (no recede), and `chrome.dim` is `None`.

- [ ] **Step 3: Add the const.** Near the other chrome constants (`:381-383`):

```rust
const CHROME_BAR_FG_BLEND: f32 = 0.18;  // bar/status fg recedes toward its panel bg — gentler than the
                                        // dropdown's 0.35 so the ladder stays Text > bar > dropdown.
```

- [ ] **Step 4: Recede the derived `Chrome` fg + set DIM.** Replace `theme.rs:298-301`:

```rust
        if self.faces.chrome == Face::default() {
            let bg = next_layer(base_bg, target);
            self.faces.chrome = Face { fg: Some(derive_fg(base_fg, bg)), bg: Some(bg), ..Face::default() };
        }
```

with:

```rust
        if self.faces.chrome == Face::default() {
            let bg = next_layer(base_bg, target);
            // E5: recede the bar/status fg toward its panel (still floor-guarded by derive_fg) + DIM,
            // so the bars read as chrome, not body text. Only Chrome changes; bg and every other face
            // are untouched, so the elevation ladder and all other pins are stable.
            let recede = blend(base_fg, (bgr, bgg, bgb), CHROME_BAR_FG_BLEND);
            self.faces.chrome = Face { fg: Some(derive_fg(recede, bg)), bg: Some(bg), dim: Some(true), ..Face::default() };
        }
```

- [ ] **Step 5: Add DIM to the non-RGB `Chrome` faces.**
  - `theme.rs:478` (terminal-plain): `chrome: Face { fg: Some(Color::White), bg: Some(Color::Black), dim: Some(true), ..Face::default() },`
  - `theme.rs:611` (terminal-ansi): add `dim: Some(true)` to its `Chrome` face literal.
  - `theme.rs:1000` (`mono_faces`, used by no-color): change `chrome: Face::default(),` to
    `chrome: Face { dim: Some(true), ..Face::default() },`.

  First confirm `mono_faces()` is used ONLY by `no_color()`:
  Run: `grep -n mono_faces wordcartel-core/src/theme.rs` — expected: definition + the single call in
  `no_color()`. If used elsewhere, do NOT edit `mono_faces`; set DIM on `no_color`'s chrome directly.

- [ ] **Step 6: Run the acceptance test — verify PASS (and tune the const if needed).**

Run: `cargo test -p wordcartel-core --lib theme::tests::e5_`
Expected: PASS. If the ladder fails on a specific theme (chrome fg pinned to the floor equals or exceeds
body fg, or drops below the dropdown), tune `CHROME_BAR_FG_BLEND` within ~[0.12, 0.22]. If NO value
satisfies every theme, STOP and raise it (the floor-vs-recede band is too tight on some theme) rather
than tuning per-theme.

- [ ] **Step 7: Re-pin the chrome-fg regression pins to the observed values.** The intentional
  derivation change makes the OLD `Chrome` fg pins stale. Run the pin test to see the actual new values:

  Run: `cargo test -p wordcartel-core --lib theme::tests::derive_chrome_base16_pins -- --nocapture`
  It will fail with the new-vs-old `c_fg` for each row. Update:
  - The `c_fg` column of every row in the pin table (`:1655-1721`, 16 rows) to the new receded value.
    Leave all other columns (bg + the four other faces) byte-identical. Add a `Chrome` dim assertion in
    the loop alongside the muted-dim one (`:1731`):
    `assert_eq!(t.face(SemanticElement::Chrome).dim, Some(true), "{label} chrome dim");`
  - Standalone Chrome-fg pins: `:1854` (tokyo), `:1917` (flexoki-dark), `:1930` (flexoki-light),
    `:1941` (zen), `:2003` (phosphor), `:2056` (synthetic light). Update ONLY the Chrome fg value on each.
  - Then grep for any remaining exact Chrome-fg assertion and re-pin:
    Run: `grep -nE 'Chrome\b' wordcartel-core/src/theme.rs | grep -iE 'assert|pin'` and inspect the
    blue-jeans exemplar / any zen pins. The grep is authoritative — the line numbers above are a floor.
  - Confirm `ChromeOverlay` fg pins are UNCHANGED (overlay still `derive_fg(base_fg, bg)` at `:323`).

- [ ] **Step 8: Full core + shell test + clippy.**

Run: `cargo test -p wordcartel-core --lib && cargo test -p wordcartel --lib && cargo clippy --workspace --all-targets`
Expected: PASS + clean. Watch the shell chrome tests (`render.rs:1526` terminal-plain status,
`render.rs:1611` tokyo chrome bg #2d2f42) — bg is unchanged so they hold; if any asserts an exact
Chrome *fg*, re-pin it.

- [ ] **Step 9: Commit.** `git commit` — "feat(theme): recede chrome bar/status fg below body text (E5)"

---

## Self-Review (completed by the plan author)

- **Spec coverage:** all six components map to Tasks 1-6; the M5 finding + dict cap + note correction are
  in Task 5; the four brainstorm forks are baked into Tasks 3/4/6/1.
- **Placeholders:** none. E5's pin RGB values are intentionally observed-then-pinned (regression pins for
  an intentional derivation change) — the acceptance test (ladder + dim) is the real TDD criterion and is
  fully specified; it iterates `Theme::builtin_names()`/`Theme::builtin(name)` (no hand-maintained
  constructor list), so it needs no per-name cross-check.
- **Type consistency:** `right_justify_leaves`'s new 4-tuple signature is used consistently in
  `grouped_commands`, the palette push, and both updated tests. `menu_leaf_parts` returns
  `(String, Option<String>)`. `MenuMark::Value(&'static str)` → `.to_string()`.

## Execution Handoff

Execute with **superpowers:subagent-driven-development** — fresh implementer per task (cheap model for
the mechanical Tasks 1/3/5; standard for 2/4; standard-to-capable for 6 with its cross-theme test and
pin table), two-verdict reviewer per task, then the Codex pre-merge gate. Ledger:
`$(git rev-parse --git-path sdd)/progress.md`.
