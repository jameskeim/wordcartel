# SRC-HI Syntax Highlighting — Design

**Status:** design, pending Codex spec review.
**Effort:** `effort-srchi-highlight` (branch off `main`).
**Fixes backlog item:** B4 (`docs/ux-backlog.md`) — `SourceHighlighted` (SRC-HI) renders identically to `SourcePlain`.

**Goal.** Make the `SourceHighlighted` (SRC-HI) render mode actually highlight: show the raw
markdown source with every construct colored in the **current theme's** element faces, while
`SourcePlain` (SOURCE) stays monochrome and `LivePreview` is untouched.

---

## Background — the bug

`RenderMode` has three variants (`editor.rs:45-48`: `LivePreview`/`SourceHighlighted`/`SourcePlain`)
and the cycle + status label treat them as three (`commands.rs:482-484` cycles LP→SH→SP;
`render.rs:348-350` labels PREVIEW/SRC-HI/SOURCE). But every place that decides *rendering*
collapses the enum to a **binary** `source_mode = mode != LivePreview` (`derive.rs:222`,
`render.rs:607`, `nav.rs:64`), and color is suppressed for `source_mode` in **two** loci:

1. **Core `md_parse::analyze(line, role, is_active)`** (`md_parse.rs:11`) short-circuits when
   `is_active` is true: `if is_active || line.is_empty()` returns the raw source as one run with
   `styles: vec![]` (`md_parse.rs:8-16`). `source_mode` forces `is_active_effective = true` for
   every line (`derive.rs:264`), so both SH and SP get raw text with no inline styles.
2. **Shell `render.rs` seg painting** (`render.rs:676-692`): for `source_mode` it composes only
   `[SE::Text]` (base canvas — no block role, no inline style); for LivePreview it composes
   `[SE::Text, role_element(vr.role), style_element(seg.style)]` (full color). (A test pins this:
   `source_mode_no_heading_fg_live_preview_has_heading_fg`, `render.rs:1658`.)

So SH and SP take the identical path and there is no "raw + styled" behaviour. SH is a labelled
third mode that renders as SOURCE.

---

## The model — stop collapsing three modes into two

`RenderMode` already has the three variants; the fix is to stop reducing them to a bool where
rendering happens. Introduce a small **line-render descriptor** with three cases (exact type is a
plan detail — a 3-variant enum, exhaustive-matchable, house style):

| Descriptor | Conceal markers? | Colorize? | Used by |
|---|---|---|---|
| `Concealed` | yes | yes | LivePreview *inactive* lines (unchanged) |
| `RawPlain` | no | no | LivePreview *active* (caret) line + all SourcePlain lines (unchanged) |
| `RawStyled` | no | yes | **all SourceHighlighted lines (NEW)** |

Notes:
- `Concealed` and `RawPlain` reproduce today's two `is_active` behaviours byte-for-byte
  (LivePreview inactive = `Concealed`; the LivePreview active line and SourcePlain = `RawPlain`).
  Only `RawStyled` is new. **LivePreview and SourcePlain must be behaviourally identical to today.**
- The LivePreview *active* line stays `RawPlain` (the existing "edit-this-line raw" reveal) — see
  Out of scope.

`derive.rs` maps `RenderMode` + per-line active-ness → descriptor:
- `LivePreview`: active line → `RawPlain`; others → `Concealed`.
- `SourceHighlighted`: every line → `RawStyled`.
- `SourcePlain`: every line → `RawPlain`.

---

## Decided design (user-ratified 2026-07-07)

1. **Uniform per-construct coloring.** In SRC-HI every markdown construct renders *entirely* in
   its theme element's color — **inline delimiters** (`**`, `_`, `` ` ``, `~~`), **block prefixes**
   (`## `, `> `, `- `), and content, all colored like the construct they belong to. Nothing plain,
   nothing split (a split — some markers colored, some plain — would read as unfinished).
2. **Reuse existing theme faces — NO new faces.** "Delimiters take the color" means they take
   *their construct's existing* face (Strong→strong, Heading→heading level, Code→code, Link→link,
   …). No new punctuation/dim `SemanticElement`. SRC-HI therefore tracks the active theme
   automatically (Style→face compose at paint; recolors on theme switch), by construction.
3. **Markers stay visible.** SRC-HI conceals nothing — the raw markers are shown, just colored.
4. **LivePreview untouched; SourcePlain untouched.**

### The two color loci the fix must reach

- **Block prefixes are free.** `render.rs` applies `role_element(vr.role)` per line to all segs
  (`render.rs:688`). Enabling the colored path for `RawStyled` gives `## ` + heading text both the
  heading color, `> ` + quote text the block-quote color, etc. — no extra machinery.
- **Inline delimiters are the one bit of new logic.** Today `analyze` puts inline markers in the
  `conceal` list and pushes `styles` only on the *content* range (`md_parse.rs:43-75`). In
  `RawStyled`, `analyze` must (a) not conceal, and (b) extend the inline style coverage to the
  **delimiter ranges** so `**` carries the same Strong style as the enclosed content (and nested
  delimiters take the nested style — the style active at the marker's position). This is the only
  new *logic*; it lives entirely in core `analyze` and only in the `RawStyled` branch, so it cannot
  perturb `Concealed`/`RawPlain`.

Render side: extend the seg-styling branch (`render.rs:676-692`, and the placed path if it has the
same `source_mode` gate) so `RawStyled` composes `[SE::Text, role_element(vr.role),
style_element(seg.style)]` (like LivePreview) rather than `[SE::Text]`, while `RawPlain` keeps
`[SE::Text]`.

---

## Invariants (reviewer-checkable)

- **Geometry ≡ SourcePlain.** SRC-HI conceals nothing, exactly like SourcePlain, so its ColMap,
  cursor stops, soft-wrap, and fold geometry are IDENTICAL to SourcePlain. Color is a pure visual
  overlay. The nav/geometry path (`nav.rs:64-67`, `nav.rs:148`; `layout::visible_width`/
  `visible_source`) is conceal-only and must pass a raw descriptor whose *conceal* behaviour equals
  today's `is_active_effective` for source mode — it may ignore the colorize bit (it discards
  styles: `let (_rows, map) = …`). No cursor/wrap/fold/ColMap change is permitted.
- **LivePreview and SourcePlain byte-identical to today** (only SH changes).
- **Cache correctness.** `LayoutKey` currently carries `source_mode: bool` (`derive.rs:10-18`), so
  SH and SP share a cached layout — switching between them would not re-layout. The key must carry
  the real mode (or the descriptor) so a mode switch invalidates the cache. This is a second face
  of the same bug and MUST be fixed or SH/SP won't differ even after the render fix.
- **Exhaustive matches** on `RenderMode` and the new descriptor — no catch-all `_`.

---

## Real-code anchors (for reviewer cross-check)

- Enum + cycle + label: `editor.rs:45-48`, `commands.rs:482-484`, `render.rs:348-350`.
- Color-suppression loci: `md_parse::analyze` `md_parse.rs:8-16` (early return) + the conceal/
  styles split `md_parse.rs:26-141`; `render.rs:676-692` seg styling (the `source_mode` gate at
  `:678`/`:685`).
- The RenderMode→bool collapse: `derive.rs:222` (+ `is_active_effective` at `:264`), `render.rs:607`,
  `nav.rs:64-65`.
- Signature-carrying functions (take `is_active`, must gain the descriptor): `md_parse::analyze`
  (`md_parse.rs:11`); `layout::layout` (`layout.rs:240-248`), `layout::visible_width`
  (`layout.rs:556-557`), `layout::visible_source` (`layout.rs:570-571`). Callers:
  `derive.rs:267`, `nav.rs:67`, `nav.rs:148` (hardcoded raw), plus tests in `md_parse.rs`/`layout.rs`.
- Cache key: `LayoutKey` `derive.rs:10-18`, built `derive.rs:234-244`.
- The test that only pinned "SH shows raw markers" (missed the collapse): `commands.rs:1212`.
- The heading-fg-per-mode test: `render.rs:1658`.

---

## Out of scope (explicit)

- **LivePreview active-line coloring.** The caret line in LivePreview stays `RawPlain` (the
  existing "edit this line" reveal). Making it `RawStyled` (keep color while revealing markers) is a
  separate LivePreview UX change, not this fix — recorded as a possible follow-up.
- **A dedicated punctuation/dim face.** Not needed (delimiters reuse their construct's face).
- **Heading glyphs / shade ramp** (`heading_level_glyph`) — untouched; orthogonal.
- **Dropping SRC-HI** (the 2-mode alternative) — rejected; we fix it.

---

## Testing & gates

- **SH ≠ SP regression** (the coverage gap that hid the bug): render an inline + a heading line in
  SH vs SP; assert SH applies the theme's Strong/Heading faces (delimiters AND content) while SP is
  monochrome base. Drive the real render (`TestBackend`) or the compose path.
- **Delimiters colored:** in SH, the `**` of `**bold**` carries the Strong face (not base), and the
  `## ` of a heading carries the heading face — the uniform-coloring decision.
- **LivePreview unchanged** and **SourcePlain unchanged** (pin both against current output).
- **Geometry identical SH vs SP:** same ColMap / cursor stops / wrap for a fixture with concealable
  markers (color must not move the cursor).
- **Cache invalidation:** cycling SH↔SP re-layouts (the `LayoutKey` carries the mode).
- GATEs: `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace
  --all-targets` clean; `cargo build`/`--no-run` warning-free for touched crates; smoke mandatory-run.
- Pipeline gates: Codex spec review (loop clean) → plan → Codex plan review (loop clean) → subagent
  execution → Codex pre-merge + Fable whole-branch → merge.

---

## Open questions the PLAN must resolve (implementation detail, not design forks)

1. The descriptor type + where it's defined (core `style`/`md_parse` vs a shared enum) and how
   `derive.rs` maps `RenderMode`+active-ness → descriptor.
2. The exact `analyze` mechanism for styling inline delimiter ranges in `RawStyled` (push style
   spans over the marker ranges using the current nesting style), without touching the `Concealed`
   path.
3. Whether the nav path passes `RawPlain` or a dedicated "geometry-only" value (it ignores color).
4. The `LayoutKey` field (store `RenderMode`, or the descriptor set) that makes SH/SP distinct.
5. The render seg-styling branch shape (3-way) + whether the *placed* path shares the same
   `source_mode` gate and needs the same treatment.
