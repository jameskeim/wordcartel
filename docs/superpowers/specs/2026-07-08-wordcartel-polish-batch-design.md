# Wordcartel polish + hygiene batch — design

**Status:** SPEC (2026-07-08). Awaiting Codex spec gate, then user review.
**Branch:** `effort-polish-batch` (off `main`).
**Nature:** a batch of six small, largely-independent items bundled into one effort to amortize the
gated-pipeline overhead. Each is its own component with its own tests; they share no code, so a problem
in one does not entangle the others.

## Goal

Ship six low-risk polish/hygiene changes as one reviewed effort:

1. **H4** — declare `pandoc` + a TeX engine as Arch `optdepends`.
2. **ux-H2** — fix the end-of-buffer active-line clamp so a just-typed final line conceals correctly.
3. **B5** — replace the H5/H6 heading gutter glyphs (which collide with the blockquote bar and list
   bullet) with a single-axis lower-block ramp for all six levels.
4. **A7** — right-justify the state VALUE in stateful menu rows into its own aligned column.
5. **M5-closeout** — cap the personal-dictionary read (the last unbounded document-class `fs::read`),
   and correct a stale note in `CLAUDE.md`.
6. **E5** — step chrome bar/status foreground intensity BELOW document body text so the bars read as
   chrome, not content.

## Non-goals / scope boundaries

- **No H1 hub refactor** (deferred until Fable credits return).
- **No Fable gate.** This batch is low-risk; Codex is the sole gate (spec → plan → pre-merge). The
  resource-behavior / instant-typing invariants are untouched (no new background work, no hot-path
  changes).
- **The M5 `fs::read` caps are already done** — see the finding below. This effort does NOT re-cap
  the recovery/fingerprint/skip-unchanged paths; it only caps the one remaining unbounded read
  (personal dictionary) and corrects the stale note.

---

## Finding that reshaped M5 (recorded for the reviewer)

`CLAUDE.md`'s "Remaining before Effort P" list says three document-sized `fs::read` paths are still
unbounded. **Verified against source (2026-07-08): all three, plus the swap read, already route
through the bounded reader** (`file::bounded_read_opt` / `swap::read_swap_capped`, capped at
`crate::limits::MAX_OPEN_BYTES`):

| CLAUDE.md-named path | Real call site | Status |
|---|---|---|
| recovery content-hash | `wordcartel/src/app.rs:1462` → `bounded_read_opt(p, MAX_OPEN_BYTES)` | capped (over-cap → `Prompt`, documented at `:1459-1461`) |
| fingerprint | `wordcartel/src/save.rs:41` → `bounded_read_opt` (over-cap → metadata-only fp) | capped + tested (`fingerprint_over_cap_falls_back_to_metadata_not_none`) |
| save skip-unchanged | `wordcartel/src/file.rs:149` → `bounded_read_opt` | capped + tested (`bounded_read_opt_caps_allocation`, `save_same_content_returns_unchanged`) |
| (swap read) | `wordcartel/src/swap.rs:185` → `Read::take(f, cap + 1)` | capped |

The note is **stale**. The only genuinely-unbounded user-influenced read left is the **personal
dictionary** at `wordcartel/src/app.rs:1449` (`std::fs::read_to_string(dict_path)`) — a config-class
file read once at startup. M5-closeout caps that one and corrects the note. Decision (user, 2026-07-08):
drop the caps-as-code component; cap the dictionary read; fix the note.

---

## Component 1 — H4: Arch `optdepends` for export

**Files:** `packaging/arch/PKGBUILD` (modify), `packaging/arch/.SRCINFO` (regenerate).

**Why:** export shells out to `pandoc` (`wordcartel/src/export.rs`), and the PDF path uses
`--pdf-engine=xelatex` (default at `wordcartel/src/config.rs:139`). Both are genuinely optional —
`probe_pandoc()` is cached and returns false when pandoc is absent; callers gate on it and surface a
status instead of failing — so these are `optdepends`, not hard `depends`.

**Change:** append two entries to the `optdepends=(...)` array at `packaging/arch/PKGBUILD:21-27`
(after the existing clipboard entries):

```sh
  'pandoc-cli: markdown export to html/docx/pdf (shells out to pandoc)'
  'texlive-xetex: PDF export via pandoc --pdf-engine=xelatex'
```

- `pandoc-cli` is the official `extra`-repo package that provides `/usr/bin/pandoc` (resolvable by
  plain `pacman`; the AUR static `pandoc-bin` is an equivalent the user may substitute).
- `texlive-xetex` provides the xelatex *format* (the engine binary lives in `texlive-bin`, but the
  format package is what `--pdf-engine=xelatex` needs; it pulls the latex/fonts formats transitively).

**Also:** regenerate `.SRCINFO` (`cd packaging/arch && makepkg --printsrcinfo > .SRCINFO`). Do NOT
hand-edit `pkgver`/`pkgrel`: this is a VCS-style package (dynamic `pkgver()` at `:35`, current
`0.0.0.r1058.g0a34a15`), so the effort's new commits auto-bump `pkgver` via `git describe` and `pkgrel`
stays `1` by convention. `.SRCINFO`'s `pkgver` will reflect whatever `git describe` yields at
regeneration time.

**Tests:** none (packaging). Verification: `makepkg --printsrcinfo` parses clean and shows the two new
optdepends; `.SRCINFO` matches the PKGBUILD. (Codex noted `.SRCINFO` is already slightly stale vs the
PKGBUILD; regenerating it here also resolves that pre-existing drift.)

**Command-surface contract:** N/A — does not touch commands, options, palette, menu, or hints.

---

## Component 2 — ux-H2: end-of-buffer active-line clamp

**Files:** `wordcartel/src/derive.rs` (modify `:279-283`), `wordcartel/src/derive.rs` tests (add one).

**Current (`derive.rs:278-283`):**
```rust
let caret_byte = b.document.selection.primary().head;
let active_line = if buf.is_empty() {
    0
} else {
    buf.byte_to_line(caret_byte.min(buf.len().saturating_sub(1)))
};
```

**Bug:** with a trailing newline and the caret at `buf.len()` (the phantom line past the final `\n`),
`caret_byte.min(buf.len()-1)` clamps back into the LAST CONTENT line, so `byte_to_line` returns that
line as active. In LivePreview the active line renders raw (`derive::line_render_for`,
`derive.rs:31` — active line = `RawPlain`, others `Concealed`), so the just-typed final list item never
shows its bullet / a final heading never shows its shade glyph until the caret moves up.

**Fix:** the `-1` in `buf.len().saturating_sub(1)` is the whole bug — it drags a caret sitting past a
trailing newline back onto the last content line. Clamp to `buf.len()` instead (which
`TextBuffer::byte_to_line` accepts and `is_char_boundary` documents as always a valid boundary). This
handles BOTH cases uniformly with no explicit trailing-newline test:

```rust
let caret_byte = b.document.selection.primary().head;
let active_line = if buf.is_empty() {
    0
} else {
    // Clamp to `len`, NOT `len-1`: a caret on the phantom line past a trailing newline must
    // map to the phantom line so the last CONTENT line conceals like any inactive line (ux-H2),
    // instead of staying "active" and rendering raw. `len` is always a char boundary.
    buf.byte_to_line(caret_byte.min(buf.len()))
};
```

Verification of the two cases (ropey `byte_to_line` counts newlines before the byte): `"a\n"`, caret at
`len=2` → `byte_to_line(2) = 1` (phantom line — the content line `"a"` now conceals). `"hello"` (no
trailing `\n`), caret at `len=5` → `byte_to_line(5) = 0` (last content line stays active — no
regression). Mid-buffer carets are `< len`, so the `min` is a no-op and behavior is byte-identical to
today.

**Interface note (Codex-corrected):** `buf` is `&wordcartel_core::buffer::TextBuffer` (a `ropey::Rope`
wrapper), **not** `ropey::Rope`. Confirmed accessors: `len()` = `rope.len_bytes()` (`buffer.rs:18`),
`is_empty()` (`buffer.rs:22`), `byte_to_line()` (`buffer.rs:78`). `TextBuffer` has **no** `ends_with`,
and `slice()` returns `String` and asserts char boundaries (`buffer.rs:64-76`) — so the earlier
`slice(len-1..len)` idea is rejected (it would panic on a buffer ending in a multibyte char). The
clamp-to-`len` fix needs none of that. `active_line` is used ONLY in an equality comparison
(`l == active_line`, `derive.rs:333`; stored in the tuple at `:306`), never as a slice/index, so a
phantom-line value that exceeds the rendered range is harmless — it simply matches no rendered line
(exactly the desired "nothing is active" for the phantom line). The plan must confirm no other
`active_line` consumer indexes with it.

**Test (new, `derive.rs` `#[cfg(test)]`):** build a buffer ending in `\n` (e.g. a one-item list
`- alpha\n`), put the caret at `buf.len()`, drive the active-line computation, and assert the last
CONTENT line is NOT the active line (so it conceals). Add the negative: caret on the content line ⇒ that
line IS active. Follow the existing `active_line_renders_raw` test shape (`derive.rs:504`).

**Command-surface contract:** N/A — rendering/derivation only.

---

## Component 3 — B5: heading gutter glyph ramp

**Files:** `wordcartel/src/render.rs` (`:18` const + test/doc updates).

**Current (`render.rs:18`):** `const SHADES: [&str; 6] = ["█", "▓", "▒", "░", "▏", "·"];`

**Problem:** H5 `▏` (U+258F, left one-eighth block) is a thin vertical left bar nearly identical to the
**blockquote** prefix glyph `▎` (U+258E; confirmed `render.rs:2320`). H6 `·` (U+00B7, middle dot) reads
as the **list bullet** `•` (U+2022; confirmed `render.rs:1968, 2122`). H1–H4 (`█▓▒░`) are a shade-density
ramp; there is no shade lighter than `░`, so the original design switched axes at rung 5 onto two glyphs
already in use elsewhere.

**Change (brainstorm decision B, 2026-07-08):** replace the whole ramp with a single-axis
**lower-block height ramp** — strictly monotone decreasing solid mass, no collision at any rung:

```rust
const SHADES: [&str; 6] = ["█", "▆", "▅", "▄", "▃", "▂"];
//                          H1   H2   H3   H4   H5   H6
// U+2588 2586 2585 2584 2583 2582
```

Painted as `format!("{shade} ")` (2-cell gutter) at `render.rs:665` and `:730` (two paint paths —
segs and placed) — unchanged; only the glyph table changes.

**Test/doc updates in `render.rs`:**
- No-color golden assertions `:2370-2375`: update each expected glyph (`█`,`▆`,`▅`,`▄`,`▃`,`▂`).
- The `text.contains('▒')` H3 assertion at `:2344` → `▅` (new H3).
- The doc-comment level→glyph map at `:2351-2356`.
- **`render.rs:1161` (Codex-found)** — a default-theme H2 test asserting the gutter `"▓ Two"` must become
  `"▆ Two"` (new H2).
- Any other assertion referencing an old SHADE glyph — grep `█ ▓ ▒ ░ ▏ ·` across `render.rs` before
  finishing and update every hit (the named sites are `:1161, :2344, :2351-2356, :2370-2375`).
- Codex noted there is no dedicated glyph-WIDTH guard test; the no-color golden already exercises that
  `▆▅▄▃▂` render as single cells in the 2-cell gutter, so no new width test is required — but if any
  golden shows a column shift, that is the signal to investigate width.

**Command-surface contract:** N/A — glyph rendering only.

---

## Component 4 — A7: right-justified value column in stateful menu rows

**Files:** `wordcartel/src/menu.rs` (`menu_leaf_base` `:59`, `right_justify_leaves` `:77`,
`grouped_commands` `:31`), `menu.rs` tests.

**Current:** `menu_leaf_base` (`:59-72`) returns one left-aligned string `"{base}: {value}"` for
stateful commands (`MenuMark::OnOff`/`MenuMark::Value`, `registry.rs:47`). `right_justify_leaves`
(`:77-94`) then pads so the CHORD ends at a common flush-right column. So the value is glued inline
after the colon and does not align across rows; only chords align.

**Target (A7, decision A):** three visual zones within a group — `[base] … [value] … [chord]` — with
the value in its own content-computed flush-right column, independent of the chord column, and stateless
rows unchanged:
```
Word Count          On     ⌘W
Line Wrap           80
Spell Check        Off     ⌘K
Clipboard         Auto
Keymap             CUA
```

**Design:** split the leaf into three parts and generalize the right-justify to two independent
right-aligned columns.

- Change `menu_leaf_base` to return the **base name and the value separately** rather than a glued
  string. Introduce a small intermediate (in `grouped_commands`) carrying `(base, Option<value>, chord,
  id)`. For stateless commands `value = None` and `base = meta.label` (unchanged). For stateful, `base`
  is the stripped label (existing logic at `:64-65`) and `value` is `"On"/"Off"` or the `Value(v)`
  string.
- Generalize `right_justify_leaves` to compute **two** targets from the group: a value-column width
  (max over `base.chars().count()`, so all values start at a common column just past the widest base)
  and the existing chord column. Right-align the value within the value column, then pad to the chord
  column as today.
- The exact column math is a spec-level detail to pin in the plan; the invariant is: (a) every value's
  right edge aligns within the group; (b) every chord's right edge aligns within the group (unchanged
  from today); (c) a row with neither renders as bare base; (d) stateless rows are byte-identical to
  today when the group has no stateful rows.
- **The both-columns case (value AND chord on one row) is a first-class requirement, not hypothetical
  (Codex).** No *default* keybinding today lands on a stateful command, but a user-customized binding
  via `chord_for()` (`keymap.rs:200`) can put a chord on a stateful row — so the layout must render
  `[base] … [value] … [chord]` correctly with both columns present, and a test must cover it.

**Preserve:** `GAP` spacing semantics; the palette entry special-case (`:46-47`); the state-in-label
CONTENT (the value is still shown — it just moves to a column). The user's explicit binding still wins
for the chord (unchanged; `chord_for`).

**Tests (`menu.rs`):**
- Update `menu_leaf_shows_state_in_label` (`:158`) — the assertion `label.starts_with("Word Count: On")`
  changes: the value is no longer glued after `: `. Assert instead that the rendered leaf contains
  `"Word Count"` and `"On"` with the value right-aligned in its column (and NOT the substring
  `"Word Count: On"`).
- Update `menu_chords_are_right_justified_within_a_group` (`:174`) if the row-construction shape changed;
  the chord flush-right invariant must still hold.
- Add a test: a group with stateful rows of differing base widths ⇒ all values share a right-edge column;
  a stateless-only group is unchanged.

**Command-surface contract:** **conforms.** A7 changes menu-row *rendering* only. State-in-label
(contract rule 8 — every stateful option shows its state in its menu label) is still honored (the value
is still displayed, relocated to a column). Menu membership, the registry-as-single-source rule,
palette exhaustiveness, every-option-has-a-command, and hint re-resolution are all untouched. The
contract's invariant tests are unaffected.

---

## Component 5 — M5-closeout: cap the dictionary read + correct the stale note

**Files:** `wordcartel/src/app.rs` (`:1447-1451`), `CLAUDE.md` (note), a test near the app load path.

**Current (`app.rs:1447-1451`):**
```rust
// Load the personal dictionary from disk (missing/unreadable → empty; no abort).
if let Some(dict_path) = &cfg.diagnostics.dictionary {
    if let Ok(text) = std::fs::read_to_string(dict_path) {
        editor.dictionary = text.lines().map(|l| l.trim().to_string()).filter(|s| !s.is_empty()).collect();
    }
}
```

**Change:** route the read through the existing bounded reader, preserving the exact degradation
("missing / unreadable / invalid-UTF-8 / over-cap → empty dictionary, no abort"):

```rust
// Load the personal dictionary from disk (missing/unreadable/over-cap → empty; no abort).
if let Some(dict_path) = &cfg.diagnostics.dictionary {
    if let Some(text) = crate::file::bounded_read_opt(dict_path, crate::limits::MAX_OPEN_BYTES)
        .and_then(|bytes| String::from_utf8(bytes).ok())
    {
        editor.dictionary = text.lines().map(|l| l.trim().to_string()).filter(|s| !s.is_empty()).collect();
    }
}
```

`bounded_read_opt` (`file.rs:133`) returns `None` on over-cap OR any read error; `String::from_utf8(..).ok()`
returns `None` on invalid UTF-8 — matching `read_to_string`'s `Err`-on-invalid → empty behavior. Net:
identical observable behavior for all in-cap valid files; over-cap now degrades to empty instead of a
multi-megabyte slurp.

**Test (new):** a within-cap dictionary file loads its words; an over-cap file (use
`bounded_read_opt`'s tested cap semantics with a tiny explicit limit if a seam exists, else assert the
`from_utf8`/`None` degradation path) yields an empty dictionary with no panic. If the load path is not
unit-testable in isolation, add the assertion at the `bounded_read_opt` seam analogous to
`bounded_read_opt_caps_allocation` (`file.rs:227`) and document that the dict path now uses it.

**CLAUDE.md correction:** in the "Remaining before Effort P" paragraph, item (2), replace the claim that
the recovery content-hash / fingerprint / save skip-unchanged reads are unbounded with the accurate
status: those are already capped via `bounded_read_opt`/`read_swap_capped`, and the dictionary read is
now capped too — **no document-class** unbounded `fs::read` remains. Word it precisely as
"document-class": Codex correctly notes that small **config/theme-class** reads remain unbounded
(`config.rs:360`, `app.rs:1430/1438` startup overrides/mask, `theme_resolve.rs:177`) — these are
deliberately out of scope (bounded-by-nature config files, read once), so the note must NOT overstate
"no unbounded reads anywhere." Keep the other item-(2) sub-point (the undo louder-hint for buffer-level
merges) if still accurate — verify before editing.

**Command-surface contract:** N/A — startup IO only.

---

## Component 6 — E5: chrome bar/status foreground recede

**Files:** `wordcartel-core/src/theme.rs` (`derive_chrome` `:298-301`; terminal/no-color constructors;
`#[cfg(test)]` pins + a new property test). No shell changes — both the menu bar (`render.rs:445`
`menu_closed`) and the normal status line (`render.rs:897`) already compose `SE::Chrome`, so receding the
`Chrome` fg recedes both.

**Current:** `derive_chrome` seeds the `Chrome` fg from `derive_fg(base_fg, bg)` (`:300`). `derive_fg`
(`:286-295`) returns `base_fg` unchanged whenever it clears the `FG_FLOOR` = 4.5 legibility floor —
which it always does on the bar — so chrome text = body text (the user's complaint). The dropdown
(`ChromeMuted`) already recedes via `blend(base_fg, base_bg, MUTED_FG_BLEND=0.35)` + `dim: Some(true)`
(`:307-310`).

**Change (brainstorm decision C — blend + DIM, gentler than the dropdown):** introduce one const and
change ONLY the `Chrome` face construction (`:298-301`):

```rust
// Elevation constants (near :383, with the other chrome constants):
const CHROME_BAR_FG_BLEND: f32 = 0.18;  // bar/status fg recedes toward its panel bg — gentler than
                                        // the dropdown's 0.35 so the ladder stays Text > bar > dropdown

// derive_chrome, the Chrome block (:298-301):
if self.faces.chrome == Face::default() {
    let bg = next_layer(base_bg, target);
    let recede = blend(base_fg, (bgr, bgg, bgb), CHROME_BAR_FG_BLEND);
    self.faces.chrome = Face { fg: Some(derive_fg(recede, bg)), bg: Some(bg), dim: Some(true), ..Face::default() };
}
```

- `blend(Color, (u8,u8,u8), f32)` mirrors the dropdown seed at `:307`. `derive_fg` still floor-guards,
  so the receded fg never drops below 4.5 against its panel.
- `dim: Some(true)` adds a graceful-degradation modifier (visible on DIM-capable terminals; the blend
  alone carries the recede where DIM is ignored).
- **Only `Chrome` changes.** `ChromeMuted`, `ChromeOverlay` (modal — primary content, stays `base_fg`),
  `ChromeSelected`, `ChromeAccent` (active prompt — stays punchy) are untouched. `bar_bg` (`:302`) and
  everything derived from it are unaffected (bg unchanged), so the elevation ladder and all other pins
  are stable. Idempotency holds (post-derive `Chrome != Face::default()`, so a second `derive_chrome`
  skips it — `:298`).

**Non-RGB paths** (base is `Color::Default` ⇒ `derive_chrome` early-returns at `:243`): add the recede
as a `dim` modifier on the explicit `Chrome` face in each affected constructor:
- `terminal-plain` (`theme.rs:478`): `chrome: Face { fg: Some(Color::White), bg: Some(Color::Black), dim: Some(true), ..Face::default() }`.
- `terminal-ansi` (`theme.rs:611`): add `dim: Some(true)` to its `Chrome` face (`White`/`DarkGray`). DIM
  is a modifier, not a color, so it respects the Ansi16 fixed-color policy.
- `no-color` uses `mono_faces()` whose `Chrome` is `Face::default()` (`theme.rs:1000`) and is not
  derived; add `dim: Some(true)` there so no-color bars recede too. Confirm `mono_faces()` is used ONLY
  by `no_color()` before editing (grep) so the DIM does not leak into another theme.

**Acceptance test (new, TDD — write first, red before the code):** iterate **all RGB builtin themes**
(Codex: don't tune to tokyo alone — the ladder must hold everywhere), each derived FULL (and spot-check
ZEN), asserting the intensity ladder per theme:
`contrast_ratio(chrome.fg, chrome.bg) < contrast_ratio(base_fg, chrome.bg)` (chrome text recedes below
what body text would be on the same panel) AND `contrast_ratio(chrome.fg, chrome.bg) >
contrast_ratio(chrome_muted.fg, chrome_muted.bg)` (bar sits above the dropdown) AND
`chrome.fg` still clears `FG_FLOOR` against `chrome.bg` AND `chrome.dim == Some(true)`. This all-themes
loop IS the tuning constraint for `CHROME_BAR_FG_BLEND` (below) — the invariant near the 4.5 floor is
fragile (Codex), so the test proves it holds on every theme rather than assuming it. Add a separate
assertion that no-color / terminal-plain / terminal-ansi `Chrome` faces carry `dim == Some(true)`.

**Pin updates (regression guards — update to the new derived values after the code lands):**
- The `derive_chrome_base16_pins` table (`theme.rs:1655-1721`, 16 rows): update the **`c_fg`** column
  (Chrome fg) on every row to the new receded value; all other columns (bg + the other four faces) stay
  byte-identical. Add `assert_eq!(t.face(Chrome).dim, Some(true), ...)` alongside the existing muted-dim
  assertion (`:1731`).
- Standalone Chrome-fg pins — the spec's original list `:1854` (tokyo FULL), `:1917` (flexoki-dark),
  `:1930` (flexoki-light) **plus the three Codex found**: `:1941` (zen pin), `:2003` (phosphor pin),
  `:2056` (synthetic light pin). `ChromeOverlay` fg pins (`:1858`, etc.) stay `base_fg` — confirmed
  unchanged (overlay still `derive_fg(base_fg,bg)` at `:323`).
- Grep for any other exact Chrome-fg assertion (blue-jeans `exemplar_spot_pins_blue_jeans`, any
  `derive_chrome_zen_*`) and re-pin only the Chrome fg — the grep is authoritative; the enumerated
  line numbers are a floor, not a ceiling.
- Shell `render.rs` chrome tests assert Chrome **bg** (`:1611` #2d2f42) and `.is_some()` — bg unchanged,
  so they hold; verify none assert an exact Chrome **fg**. `terminal_plain_status_carries_chrome_face`
  (`:1526`) asserts bg=Black (not reverse) — DIM is orthogonal; confirm it still passes.

**Method for the new pin values:** these are regression pins for an intentional derivation change — after
implementing, run the test, read the actual receded `c_fg` values, and update the table to match. The
NEW all-themes property test (ladder ordering + dim) is the real acceptance criterion; the pin table is
the guard against future accidental drift. Choose `CHROME_BAR_FG_BLEND` (start ~0.18, tune within
~[0.12, 0.22]) as whatever value satisfies the all-themes ladder test; if no single value clears the
ladder on every builtin, that is a finding to raise (it would mean the floor-vs-recede band is too
tight on some theme) rather than silently tuning per-theme.

**Command-surface contract:** N/A — theme foreground derivation only.

---

## Effort structure & gating

- **One branch:** `effort-polish-batch` off `main`.
- **Execution:** subagent-driven, one implementer + one two-verdict reviewer per component (TDD:
  failing test → impl → green → commit). Order trivial→design so the low-risk five land first and E5
  (the only design-surface item) is last and separable — if E5 needs iteration it can defer to a
  follow-up without holding the other five: **H4 → ux-H2 → B5 → A7 → M5-closeout → E5.**
- **Gates:** Codex spec gate (this doc) → plan → Codex plan gate → SDD per-component review → Codex
  pre-merge GO/NO-GO. **No Fable** (low-risk; credit-blocked until the 11th regardless).
- **Merge:** `--no-ff` to `main`, verify `cargo test` + workspace clippy on the merged result, then
  confirm with the human before merging/pushing (commit/push only when asked).

## Global constraints (apply to every component)

- `cargo test` green across all suites; `cargo build` + `cargo test --no-run` warning-free for touched
  crates; **workspace clippy clean is a GATE** (`cargo clippy --workspace --all-targets`).
- Do **not** run `cargo fmt`; hand-match the dense house style (em-dash `—` in prose, no emoji in code,
  4-space indent, hand-wrapped ~100 cols).
- `#![forbid(unsafe_code)]` holds (no unsafe anywhere).
- PTY smoke suite (`scripts/smoke/run.sh`) is mandatory-run / advisory-pass — quote its one-line summary
  in the pre-merge report.
- No new `.unwrap()` on fallible/external paths; typed errors to the status line, never the console.
- Every commit ends with the project trailers verbatim (`Co-Authored-By: Claude Opus 4.8 (1M context)`
  + `Claude-Session:`).

## Command-surface conformance (summary)

Only **A7** touches the command/menu surface, and it **conforms**: it changes menu-row rendering only
(value relocated to an aligned column), preserving state-in-label, registry-as-single-source, palette
exhaustiveness, menu ⊆ palette, and hint re-resolution. The contract's invariant tests are unaffected.
H4, ux-H2, B5, M5-closeout, E5 are all **N/A** (packaging / rendering / startup IO / theme derivation —
no command, option, palette, menu-membership, or hint change).
