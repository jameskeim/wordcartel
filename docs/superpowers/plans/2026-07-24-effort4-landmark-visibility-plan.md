# Effort ④ — Landmark Visibility Implementation Plan

**Spec:** `docs/superpowers/specs/2026-07-24-effort4-landmark-visibility-design.md`
(Codex-gated READY, round 3 clean). **Branch:** `effort-4-landmark-visibility` off main @
`6067ad5` (already created; the spec is its only commit content so far).
**Anchors are SYMBOL names** — re-locate by name (`workspaceSymbol`/grep), never trust a
line number. For compile/usage questions on code you are editing, trust `cargo` + `grep`,
NOT editor/LSP "unused"/"undefined" hints (they lag edits; the controller's diagnostics
about your files are the most stale).

## Global Constraints

- **Commit only when a task says commit; push never.** Every commit message ends with the
  two project trailers, verbatim:

  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01FAx3iA5vRBiLXEfCnudR6j
  ```

- **Do NOT run `cargo fmt`** (no rustfmt.toml; the tree is hand-formatted — match
  neighbors by hand; never reflow untouched code).
- **Merge GATEs** (every task ends green; Task 7 re-verifies all): `cargo test` green
  across all suites; `cargo build` and `cargo test --no-run` warning-free for touched
  crates; `cargo clippy --workspace --all-targets` clean; `module_budgets` green with
  **`render.rs` staying strictly below 899 production lines** — "production lines" in
  the `module_budgets::production_lines` sense (lines before the file's `mod tests`;
  899/900 at baseline), NOT raw `wc -l` (which is ~4286 for this file). ④ must be
  net-negative there; NO budget bump.
- **`wordcartel-core` stays `#![forbid(unsafe_code)]`** — the one core addition
  (`TextBuffer::byte`) is safe rope delegation.
- **Perf law:** paint work is per-visible-row; `BlockPaint` is gathered ONCE per frame;
  no per-keystroke O(document) anywhere (mark windowing is O(#marks) per frame, #marks
  tiny; per-glyph checks are binary-search/short-scan).
- **LOCKED decisions (do not re-open; a conflict is a HUMAN decision):** 1B — ALL
  landmarks visible (block + char marks/bookmarks); quiet interior tint + strong
  boundary cells; ONE `SE::MarkedBlockBoundary` (begin, end, AND the pending anchor);
  `SE::LandmarkGlyph` for mark presence cells; end boundary = last interior glyph
  (`b.end - 1`); identity via presence cells + `· MK <ids>` caret-line status segment
  (no injected glyphs — B-lite, zero ColMap/layout impact); C2 folded in (`clear_mark` +
  `clear_marks`, the ONLY new commands); visibility ALWAYS-ON (no option/toggle);
  precedence at a shared cell block-boundary > pending > landmark > interior with
  EXCLUSIVE classification (never stack landmark faces); undo drift ACCEPTED +
  documented (marks stale-but-clamped after undo — never cleared, never remapped);
  cue allocation — interior `reverse+dim` (mono) / bg-tint (RGB), boundary
  `reverse+bold+underline`, landmark `reverse+italic+underline`, all ADDITIVE (H25
  stays out).
- **Command-surface contract:** `clear_mark`/`clear_marks` are registry builtins (law 1),
  palette-exhaustive by construction (law 3 — the palette enumerates the registry; the
  existing invariant covers rows that EXIST, and T6 adds a presence test so a MISSING
  row is also caught), `menu: None` (`CommandMeta.menu: Option<MenuCategory>`) matching
  all 24 existing mark commands
  (law 4 curation: the marks' home is the palette; menu unchanged), no default chord
  (palette is the keyboard path; a user config-patch binding gets hints via the
  generic `Keymap::bind_user` machinery — law 7). Always-on visibility adds NO
  user-settable option, so no option=command obligation arises (law 2).

## File Map

| File | Change |
|---|---|
| `wordcartel-core/src/buffer.rs` | + `TextBuffer::byte` accessor (+ doc example + unit test) |
| `wordcartel-core/src/theme.rs` | + 2 `SemanticElement`s, `ThemeFaces` fields, `face`/`face_mut`/`element_from_key` arms, 7 constructor sites, interior modifier-drop; contract-test updates |
| `wordcartel/src/block_paint.rs` | NEW — the ④ seam (gather/classify/patch/trailing/status helpers) + unit tests |
| `wordcartel/src/lib.rs` | + `pub mod block_paint;` |
| `wordcartel/src/render.rs` | `RowCtx` embeds `BlockPaint`; inline block arm → `patch_glyph`; `last_row` plumbing + trailing call; NET-SHRINK |
| `wordcartel/src/render_status.rs` | BLK direction / `BLK…` / `MK` segments |
| `wordcartel/src/editor.rs` | `MarkPending` + `Clear` |
| `wordcartel/src/marks.rs` | `clear_mark`/`clear_marks` + `resolve_pending` Clear arm |
| `wordcartel/src/registry.rs` | 2 command rows |
| `wordcartel/src/e2e.rs` | 1 journey |

## Task Order & Rationale

1. **T1 core** (`TextBuffer::byte`) — leaf dependency of Task 3's `logical_content_end`.
2. **T2 theme** (elements + faces + contracts) — leaf dependency of Tasks 3-5; the
   compiler's exhaustive-match errors drive the census.
3. **T3 block_paint** (the seam, pure logic + unit vectors) — depends on T1+T2; nothing
   calls it yet, so the tree stays green mid-effort.
4. **T4 render** (integration; net-shrink) — first visible behavior change.
5. **T5 status** (segments) — depends on T3 helpers.
6. **T6 C2** (clear commands) — independent of paint; last code task.
7. **T7 probes + e2e + final verification** — residual-flag probes (clipping/B17;
   live legibility), the e2e journey, full gates. No merge.

Each intermediate state compiles and is fully green at its commit.

---

### Task 1: `TextBuffer::byte` (wordcartel-core)

**RED.** Add to the existing `#[cfg(test)] mod tests` in `wordcartel-core/src/buffer.rs`:

```rust
    /// ④: byte-level accessor — safe at ANY offset including mid-codepoint (the
    /// motivating read: `slice(1..2)` on "é" trips the char-boundary assert).
    #[test]
    fn byte_reads_raw_bytes_including_mid_codepoint() {
        let buf = TextBuffer::from_str("é\n");
        assert_eq!(buf.byte(0), 0xC3);
        assert_eq!(buf.byte(1), 0xA9); // continuation byte of U+00E9
        assert_eq!(buf.byte(2), b'\n');
    }
```

Baseline state: `TextBuffer` has no `byte` method (public surface:
`from_str/len/is_empty/clamp_to_boundary/insert/delete/slice/byte_to_line/line_to_byte/
caret_line_col/snapshot`). Red = **compile error E0599** (`no method named byte`) —
`cargo test -p wordcartel-core buffer` fails to build. (Named-baseline red: a
new-API test's red is the compile failure; there is no runtime-red possible.)

**GREEN.** In `impl TextBuffer` (place directly after `byte_to_line` — it is the sibling
byte-indexed reader):

```rust
    /// Returns the raw byte at offset `i` (`0 <= i < self.len()`). Byte-level accessor
    /// for single-byte sentinels (`\n`) — safe at ANY offset including mid-codepoint,
    /// unlike [`TextBuffer::slice`], which asserts char boundaries.
    ///
    /// # Panics
    /// Panics if `i >= self.len()` (ropey bounds check).
    ///
    /// # Examples
    /// ```
    /// use wordcartel_core::buffer::TextBuffer;
    ///
    /// let buf = TextBuffer::from_str("é\n");
    /// assert_eq!(buf.byte(1), 0xA9); // mid-codepoint — slice(1..2) would panic
    /// assert_eq!(buf.byte(2), b'\n');
    /// ```
    pub fn byte(&self, i: BytePos) -> u8 {
        self.rope.byte(i)
    }
```

(`ropey::Rope::byte` exists in the pinned ropey 1.6.1 — verified in the registry source.)

**Verify:** `cargo test -p wordcartel-core` (unit + the new doctest) green;
`cargo clippy -p wordcartel-core --all-targets` clean.

**Commit:** `effort4 T1: core TextBuffer::byte — byte-level accessor (UTF-8-safe newline probe)` + trailers.

---

### Task 2: Theme — `MarkedBlockBoundary` + `LandmarkGlyph` + the quiet interior

**RED.** In `wordcartel-core/src/theme.rs`'s `#[cfg(test)] mod tests`, make FOUR edits
FIRST (baseline: the variants do not exist → red = compile errors E0599/E0433 across the
test mod; edit 4 stays runtime-red even after the variants exist, until the keys are
wired):

1. Extend the totality array (locate `const ALL_ELEMENTS`): change its type to
   `[SemanticElement; 37]`, insert `MarkedBlockBoundary, LandmarkGlyph` immediately after
   `MarkedBlock` in the list, and update the count comment:
   `// 37 = Text + 6 inline + 6 heading + 4 block + 6 (fm/comment/sel/marked-block/boundary/landmark) + 7 overlay + 6 chrome + 1 prose-lens.`
2. In `face_requirement`, add an arm (keep the match exhaustive, no `_`):

```rust
            MarkedBlockBoundary | LandmarkGlyph                               => Modifier,
```

3. REPLACE the test `marked_block_mono_modifier_is_distinct` (its interior pin —
   reverse+bold+underline — is deliberately falsified by ④) with:

```rust
    // a11y (④): the landmark trio is mono-distinct — quiet interior (reverse+dim),
    // strong boundary (reverse+bold+underline: the pre-④ interior combo, reassigned),
    // landmark glyph (reverse+italic+underline). Pairwise distinct from each other and
    // from every co-occurring highlight/cue face in the near-full mono space.
    #[test]
    fn landmark_mono_modifiers_are_distinct() {
        let t = no_color();
        let interior = t.face(SemanticElement::MarkedBlock);
        assert_eq!((interior.reverse, interior.dim), (Some(true), Some(true)));
        assert_eq!((interior.bold, interior.underline), (None, None), "interior is QUIET");
        let boundary = t.face(SemanticElement::MarkedBlockBoundary);
        assert_eq!((boundary.reverse, boundary.bold, boundary.underline),
                   (Some(true), Some(true), Some(true)));
        let landmark = t.face(SemanticElement::LandmarkGlyph);
        assert_eq!((landmark.reverse, landmark.italic, landmark.underline),
                   (Some(true), Some(true), Some(true)));
        let peers = [SemanticElement::Selection, SemanticElement::SearchMatch,
                     SemanticElement::SearchCurrent, SemanticElement::DiagSpelling,
                     SemanticElement::DiagGrammar, SemanticElement::ProseLensMatch,
                     SemanticElement::FrontMatter, SemanticElement::Comment];
        for f in [interior, boundary, landmark] {
            for p in peers { assert_ne!(f, t.face(p), "{p:?} must stay distinct"); }
        }
        assert_ne!(interior, boundary);
        assert_ne!(boundary, landmark);
        assert_ne!(interior, landmark);
        assert!(ALL_ELEMENTS.contains(&SemanticElement::MarkedBlockBoundary));
        assert!(ALL_ELEMENTS.contains(&SemanticElement::LandmarkGlyph));
    }
```

4. (Codex plan-gate round 1, finding 1) Extend the EXISTING test
   `element_from_key_maps_snake_case_names` with the two new keys — it checks only
   specific keys, so a forgotten `element_from_key` arm would otherwise pass silently:

```rust
        assert_eq!(element_from_key("marked_block_boundary"), Some(MarkedBlockBoundary));
        assert_eq!(element_from_key("landmark_glyph"), Some(LandmarkGlyph));
```

   Red integrity: compile-red at baseline (variants absent); if the GREEN phase wired
   the enum but forgot the `element_from_key` arms, this stays RUNTIME-red
   (`element_from_key` returns `None`) — the registration-gap coverage the plain
   compile-red edits cannot provide.

**GREEN.** Production edits, all in `theme.rs`:

1. `SemanticElement` enum — after the `MarkedBlock` variant:

```rust
    /// The begin/end boundary cells of a marked block (and the pending ^KB anchor cell).
    MarkedBlockBoundary,
    /// A char-mark / numbered-bookmark presence cell (landmark visibility, Effort ④).
    LandmarkGlyph,
```

2. `struct ThemeFaces` — extend the `selection`/`marked_block` line:

```rust
    front_matter: Face, comment: Face, selection: Face, marked_block: Face,
    marked_block_boundary: Face, landmark_glyph: Face,
```

3. `Theme::face` — after the `MarkedBlock => self.faces.marked_block,` arm:

```rust
            MarkedBlockBoundary => self.faces.marked_block_boundary,
            LandmarkGlyph => self.faces.landmark_glyph,
```

4. `Theme::face_mut` — after `MarkedBlock => &mut self.faces.marked_block,`:

```rust
            MarkedBlockBoundary => &mut self.faces.marked_block_boundary,
            LandmarkGlyph => &mut self.faces.landmark_glyph,
```

5. `element_from_key` — after `"selection" => Selection, "marked_block" => MarkedBlock,`:

```rust
        "marked_block_boundary" => MarkedBlockBoundary, "landmark_glyph" => LandmarkGlyph,
```

6. **The seven constructor sites.** At each, EDIT the `marked_block:` line (keep the bg,
   drop `reverse`/`bold`/`underline`) and ADD the two new lines directly below it. The
   two new faces are the SAME literal at all six colored sites:

```rust
            marked_block_boundary: Face { reverse: Some(true), bold: Some(true), underline: Some(true), ..Face::default() },
            landmark_glyph: Face { reverse: Some(true), italic: Some(true), underline: Some(true), ..Face::default() },
```

   Per-site interior (comment updated to `// ④ landmark visibility: quiet interior tint — strong cues live on the boundary face.`):
   - `default()` (also serves terminal-plain): `marked_block: Face { bg: Some(Color::DarkGray), ..Face::default() },`
   - `tokyo_night()`: `marked_block: Face { bg: Some(DARK3), ..Face::default() },`
   - `terminal_ansi()`: `marked_block: Face { bg: Some(Color::DarkGray), ..Face::default() },`
   - `blue_jeans(name, r)`: `marked_block: Face { bg: Some(r.mark_bg), ..Face::default() },`
   - `from_base16(name, p)`: `marked_block: Face { bg: Some(b[0x3]), ..Face::default() },`
   - `phosphor(name, hue)`: `marked_block: Face { bg: Some(shade(hue, 2)), ..Face::default() },`
   - `mono_faces()` (no bg available; `modface`/`m` has no dim param, so the interior is
     a literal; boundary/landmark use `m(bold, italic, underline, strike, reverse)`):

```rust
        marked_block: Face { reverse: Some(true), dim: Some(true), ..Face::default() }, // ④ quiet interior (r+d)
        marked_block_boundary: m(true, false, true, false, true),  // reverse+bold+underline (the retired interior combo)
        landmark_glyph: m(false, true, true, false, true),         // reverse+italic+underline
```

7. Sweep any remaining exhaustive-match sites the compiler flags (`cargo build -p
   wordcartel-core` then `cargo build --workspace`) — the spec's census found none
   outside `theme.rs` (`compose.rs` folds over faces, `theme_resolve.rs` goes through
   `element_from_key`), but the compiler is the authority; add thin arms only, never a
   `_` catch-all. Also update the one test constructing a bare `ThemeFaces` literal
   (`grep -n 'ThemeFaces {' theme.rs` in the test mod — the `marked_block:
   Face::default()` fixture) with the two new `Face::default()` fields.
8. **Migrate the two render.rs tests that pin the OLD block look** (Codex plan-gate
   round 2 — T2 changes the look, so T2 owns the pins; without this, `cargo test -p
   wordcartel` is red between T2 and T4, breaking the intermediate-green invariant).
   The old assertions ride `row_has_highlight`, which recognizes only a Yellow bg or
   `Modifier::REVERSED` — the quiet interior (per-theme tint bg, no REVERSED) is
   invisible to it. Do NOT widen the helper (it serves the search/selection tests with
   exactly those cues); instead the two block tests assert the SPECIFIC new interior
   form directly — still meaningful (they fail if the interior paint were dropped:
   the tint-bg equality breaks), and stable through T4 (they assert a strictly-INTERIOR
   cell, col 2, which stays `Interior` after the boundary split; T4's own new test pins
   the boundary cells). Full replacement bodies:

```rust
    #[test]
    fn marked_block_paints_and_status_shows_blk() {
        let mut e = Editor::new_from_text("hello world\n", None, (60, 6));
        e.status_line_mode = crate::config::TransientMode::On; // test chrome status content, not calm mode
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false });
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 60, 6);
        // ④ quiet interior: a strictly-interior cell (col 2 — stable through the T4
        // boundary split) carries the theme's MarkedBlock tint bg and is NOT reversed.
        let want_bg = crate::compose::face_to_ratatui(&e.theme.face(SE::MarkedBlock), e.depth).bg;
        assert!(want_bg.is_some(), "default theme's interior tint has a bg");
        assert_eq!(buf[(2u16, 0u16)].style().bg, want_bg, "interior cell carries the tint");
        assert!(!buf[(2u16, 0u16)].style().add_modifier.contains(Modifier::REVERSED),
            "④: the interior is QUIET — the pre-④ reversed look is retired");
        assert_ne!(buf[(7u16, 0u16)].style().bg, want_bg, "outside the block: no tint");
        // and the status row contains "BLK"
        assert!(row_string(&buf, 5).contains("BLK"), "status shows BLK indicator");
    }

    #[test]
    fn hidden_block_status_reads_blk_hidden_and_not_painted() {
        let mut e = Editor::new_from_text("hello\n", None, (60, 6));
        e.status_line_mode = crate::config::TransientMode::On; // test chrome status content, not calm mode
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: true });
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 60, 6);
        assert!(row_string(&buf, 5).contains("BLK·hidden"));
        // ④: "not painted" pinned against the ACTUAL interior form — a leaked hidden
        // block would tint row 0 with the MarkedBlock bg (`row_has_highlight` cannot
        // see the quiet interior, so assert the face directly — no vacuous pass).
        let tint = crate::compose::face_to_ratatui(&e.theme.face(SE::MarkedBlock), e.depth).bg;
        assert!((0..10u16).all(|x| buf[(x, 0u16)].style().bg != tint),
            "hidden block paints no interior tint");
    }
```

   (`SE`, `Modifier`, `row_string`, `render_to_buffer` are already in scope in
   render.rs's test mod via its existing imports/`use super::*`. At T2 these pass
   because the OLD inline render arm still whole-span-patches the — now bg-only —
   `SE::MarkedBlock` face; at T4 they keep passing because col 2 remains classified
   `Interior`. Census of other old-look pins: theme.rs's mono test + `ThemeFaces`
   fixture are already migrated by edits 3/7 above; the render.rs face-distinctness
   battery compares whole `Face` values (`ProseLensMatch` vs `MarkedBlock` — bgs stay
   distinct per theme) and survives; e2e.rs / compose.rs / theme_picker.rs / base16.rs
   contain NO MarkedBlock styling assertions — grep-verified.)

**Verify:** `cargo test -p wordcartel-core` — the rewritten mono test, totality (37),
the extended `element_from_key` test,
`every_rgb_builtin_satisfies_the_completeness_contract` (the two Modifier-class faces
carry modifiers; every interior keeps a bg), and
`prose_lens_bg_distinct_from_marked_block_in_color_themes` (no interior bg changed) all
green. **`cargo test --workspace` green** — the intermediate-green invariant: the two
migrated render.rs pins (step 8) pass against the old render arm + new faces.
`cargo build --workspace` warning-free (catches any exhaustive site outside core).
`cargo clippy --workspace --all-targets` clean.

**Commit:** `effort4 T2: theme — MarkedBlockBoundary + LandmarkGlyph elements; quiet block interior` + trailers.

---

### Task 3: `block_paint.rs` — the seam (pure logic + all worked vectors)

**RED.** Create the module WITH its test mod first and register it; baseline red is the
missing production items (E0599 on the fns as the test mod references them — write tests
referencing the final API, stub nothing). Concretely: add `pub mod block_paint;` to
`wordcartel/src/lib.rs` (alphabetical-ish near `pub mod base16;`/`pub mod chrome_geom;`
— match the file's grouping style), create `wordcartel/src/block_paint.rs` containing
ONLY the module doc + the `#[cfg(test)] mod tests` below, run
`cargo test -p wordcartel block_paint` → compile red. Then fill production code → green.

Production code (complete):

```rust
//! Landmark visibility (Effort ④): the ONE seam for marked-block interior/boundary/
//! pending paint, char-mark/bookmark presence cells, and the landmark status helpers.
//! B-lite: styles EXISTING cells only — no injected glyphs, no ColMap/layout impact.
//! Classification is EXCLUSIVE per cell (block boundary > pending > landmark >
//! interior) — landmark faces never stack, so add-only compose cannot accumulate
//! modifier soup in the near-full mono cue-space.

use ratatui::style::Style as RStyle;
use ratatui::text::Span;
use wordcartel_core::buffer::TextBuffer;
use wordcartel_core::theme::{Depth, SemanticElement as SE, Theme};
use crate::editor::{Editor, MarkedBlock};
use crate::render::overlaps;

/// Per-frame landmark snapshot — gathered ONCE in `gather_row_ctx`, carried in `RowCtx`.
pub(crate) struct BlockPaint {
    /// The completed block, already filtered to visible (`!hidden`).
    block: Option<MarkedBlock>,
    /// The ^KB anchor awaiting ^KK.
    pending: Option<usize>,
    /// Mark positions (values of `Buffer.marks`), sorted ascending, deduplicated.
    landmarks: Vec<usize>,
}

/// Exclusive cell classification (locked decision 8). Exactly ONE face patches per cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum CellKind { Boundary, Pending, Landmark, Interior }

/// Gather the active buffer's landmark state. O(#marks log #marks), once per frame.
pub(crate) fn gather(editor: &Editor) -> BlockPaint {
    let b = editor.active();
    let mut landmarks: Vec<usize> = b.marks.values().copied().collect();
    landmarks.sort_unstable();
    landmarks.dedup();
    BlockPaint { block: b.marked_block.filter(|mb| !mb.hidden), pending: b.pending_block_begin, landmarks }
}

/// The logical content end of `line`: the newline byte's own offset, or `buf.len()` on
/// a final line without one. BYTE-level newline test — `slice` asserts char boundaries
/// and would panic on a multibyte final char (spec §3.3, Codex round 2).
pub(crate) fn logical_content_end(buf: &TextBuffer, line: usize) -> usize {
    let start = crate::derive::line_start(buf, line);
    let next = crate::derive::line_start(buf, line + 1); // clamps to buf.len() past the last line
    if next > start && buf.byte(next - 1) == b'\n' { next - 1 } else { next }
}

impl BlockPaint {
    /// Whether the placed row builder is required. Fixes the B12 gate gap: a lone
    /// pending anchor, or bare marks, now force the placed path.
    pub(crate) fn wants_placed(&self) -> bool {
        self.block.is_some() || self.pending.is_some() || !self.landmarks.is_empty()
    }

    /// First-match-wins exclusive classification for the glyph `[g_from, g_to)`.
    fn classify(&self, g_from: usize, g_to: usize) -> Option<CellKind> {
        if let Some(b) = self.block {
            // b.start < b.end is a model invariant (`set_block` rejects empty;
            // `Buffer::apply` clears collapsed) — `b.end - 1` cannot underflow past start.
            if overlaps(g_from, g_to, b.start, b.start + 1)
                || overlaps(g_from, g_to, b.end - 1, b.end) {
                return Some(CellKind::Boundary);
            }
        }
        if let Some(p) = self.pending {
            if overlaps(g_from, g_to, p, p + 1) { return Some(CellKind::Pending); }
        }
        let i = self.landmarks.partition_point(|&m| m < g_from);
        if self.landmarks.get(i).is_some_and(|&m| m < g_to) { return Some(CellKind::Landmark); }
        if let Some(b) = self.block {
            if overlaps(g_from, g_to, b.start, b.end) { return Some(CellKind::Interior); }
        }
        None
    }

    /// The per-glyph patch — replaces render.rs's inline MarkedBlock arm. ONE face,
    /// chosen by exclusive classification; add-only compose stays sound.
    pub(crate) fn patch_glyph(&self, style: RStyle, g_from: usize, g_to: usize,
                              theme: &Theme, depth: Depth) -> RStyle {
        let el = match self.classify(g_from, g_to) {
            Some(CellKind::Boundary | CellKind::Pending) => SE::MarkedBlockBoundary,
            Some(CellKind::Landmark) => SE::LandmarkGlyph,
            Some(CellKind::Interior) => SE::MarkedBlock,
            None => return style,
        };
        style.patch(crate::compose::face_to_ratatui(&theme.face(el), depth))
    }

    /// EOL/empty-line marker cell for an entry's LAST visual row (spec §3.3): fires iff
    /// a marker byte equals the entry's final logical line's content end — a position
    /// never covered by a placed glyph. Priority Boundary > Pending > Landmark; a
    /// one-glyph-at-EOL block resolves end-first (`]`).
    pub(crate) fn trailing_marker(&self, editor: &Editor, l: usize) -> Option<Span<'static>> {
        let b = editor.active();
        let buf = &b.document.buffer;
        let end_line = crate::ventilate::resolve(&b.view, buf, l)
            .map(|r| r.last_line).unwrap_or(l);
        let cend = logical_content_end(buf, end_line);
        let (glyph, el) = if self.block.is_some_and(|mb| mb.end - 1 == cend) {
            ("]", SE::MarkedBlockBoundary)
        } else if self.block.is_some_and(|mb| mb.start == cend) || self.pending == Some(cend) {
            ("[", SE::MarkedBlockBoundary)
        } else if self.landmarks.binary_search(&cend).is_ok() {
            ("·", SE::LandmarkGlyph)
        } else {
            return None;
        };
        let style = crate::compose::face_to_ratatui(&editor.theme.face(el), editor.depth);
        Some(Span::styled(glyph.to_string(), style))
    }
}

/// Directional off-screen hint for the (non-hidden) block — line-granular (spec §3.4).
pub(crate) fn blk_direction(editor: &Editor, b: MarkedBlock) -> &'static str {
    let buf = &editor.active().document.buffer;
    let view = &editor.active().view;
    let Some((&max_line, _)) = view.line_layouts.last_key_value() else { return "" };
    let lo = crate::derive::line_start(buf, view.scroll);
    let end_line = crate::ventilate::resolve(view, buf, max_line)
        .map(|r| r.last_line).unwrap_or(max_line);
    let hi = crate::derive::line_start(buf, end_line + 1);
    if b.end <= lo { "↑" } else if b.start >= hi { "↓" } else { "" }
}

/// `MK <ids>` for marks on the caret's line (identity segment, spec §3.4). Stale
/// (post-undo) positions are CLAMPED before `byte_to_line` — a drifted mark past EOF
/// must never panic the status path (accepted undo drift, spec §4.2).
pub(crate) fn marks_on_caret_line(editor: &Editor) -> Option<String> {
    let b = editor.active();
    if b.marks.is_empty() { return None; }
    let buf = &b.document.buffer;
    let line = buf.byte_to_line(crate::nav::head(editor));
    let ids: Vec<String> = b.marks.iter()
        .filter(|&(_, &pos)| buf.byte_to_line(pos.min(buf.len())) == line)
        .map(|(c, _)| c.to_string())
        .collect();
    if ids.is_empty() { None } else { Some(format!("MK {}", ids.join(","))) }
}
```

Test mod (complete — these ARE the spec's worked vectors):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::{Editor, MarkedBlock};
    use wordcartel_core::buffer::TextBuffer;

    fn bp(block: Option<MarkedBlock>, pending: Option<usize>, marks: &[usize]) -> BlockPaint {
        let mut landmarks = marks.to_vec();
        landmarks.sort_unstable(); landmarks.dedup();
        BlockPaint { block, pending, landmarks }
    }
    fn blk(start: usize, end: usize) -> Option<MarkedBlock> {
        Some(MarkedBlock { start, end, hidden: false })
    }

    // --- logical_content_end: the spec §3.3 vectors, verbatim ---
    #[test]
    fn content_end_trailing_newline() {
        let buf = TextBuffer::from_str("abc\n");
        assert_eq!(logical_content_end(&buf, 0), 3); // the newline's own offset
    }
    #[test]
    fn content_end_concealed_markup_is_not_the_visible_end() {
        // `**a**\n`: visible content ends at byte 3, but the LOGICAL content end is 5
        // (the newline) — a concealed-byte mark at 3 is NOT at cend (spec round-1 fold).
        let buf = TextBuffer::from_str("**a**\n");
        assert_eq!(logical_content_end(&buf, 0), 5);
    }
    #[test]
    fn content_end_final_line_without_newline() {
        let buf = TextBuffer::from_str("abc");
        assert_eq!(logical_content_end(&buf, 0), 3); // == len
    }
    #[test]
    fn content_end_multibyte_eof_does_not_panic() {
        // Round-2 vector: "é" = 2 bytes; byte(1) is the U+00E9 continuation byte —
        // the old slice(1..2) would trip the char-boundary assert.
        let buf = TextBuffer::from_str("é");
        assert_eq!(logical_content_end(&buf, 0), 2);
    }
    #[test]
    fn content_end_empty_line() {
        let buf = TextBuffer::from_str("\n");
        assert_eq!(logical_content_end(&buf, 0), 0);
    }
    #[test]
    fn content_end_past_last_line_clamps() {
        let buf = TextBuffer::from_str("abc\n");
        // rope line 1 is the empty final line: start == next == len → cend == len.
        assert_eq!(logical_content_end(&buf, 1), 4);
    }

    // --- classification: priority + exclusivity ---
    #[test]
    fn boundary_wins_over_landmark_and_interior() {
        let p = bp(blk(0, 5), None, &[0, 2]);
        assert_eq!(p.classify(0, 1), Some(CellKind::Boundary)); // mark at b.start loses
        assert_eq!(p.classify(4, 5), Some(CellKind::Boundary)); // end = last interior glyph
        assert_eq!(p.classify(2, 3), Some(CellKind::Landmark)); // mark inside beats interior
        assert_eq!(p.classify(1, 2), Some(CellKind::Interior));
        assert_eq!(p.classify(6, 7), None);
    }
    #[test]
    fn pending_beats_landmark_loses_to_boundary() {
        let p = bp(blk(0, 2), Some(1), &[1]);
        assert_eq!(p.classify(1, 2), Some(CellKind::Boundary)); // b.end-1 == 1 wins over pending
        let p2 = bp(None, Some(3), &[3]);
        assert_eq!(p2.classify(3, 4), Some(CellKind::Pending));
    }
    #[test]
    fn one_glyph_block_is_a_single_boundary_cell() {
        let p = bp(blk(2, 3), None, &[]);
        assert_eq!(p.classify(2, 3), Some(CellKind::Boundary));
    }
    #[test]
    fn wants_placed_gates() {
        assert!(!bp(None, None, &[]).wants_placed());
        assert!(bp(blk(0, 2), None, &[]).wants_placed());
        assert!(bp(None, Some(0), &[]).wants_placed()); // the B12 gap, closed
        assert!(bp(None, None, &[7]).wants_placed());
    }
    #[test]
    fn gather_filters_hidden_and_sorts_marks() {
        let mut e = Editor::new_from_text("hello world\n", None, (40, 10));
        e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 5, hidden: true });
        e.active_mut().marks.insert('b', 7);
        e.active_mut().marks.insert('a', 2);
        e.active_mut().marks.insert('c', 7); // dup position
        let p = gather(&e);
        assert!(p.block.is_none(), "hidden block filtered at gather");
        assert_eq!(p.landmarks, vec![2, 7], "sorted + deduped");
    }

    // --- trailing marker (via a real Editor; LivePreview is the default mode) ---
    #[test]
    fn trailing_fires_for_eol_mark_and_not_concealed_byte() {
        let mut e = Editor::new_from_text("**a**\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        // concealed trailing `*` at byte 3: cend is 5 → NO cell.
        e.active_mut().marks.insert('x', 3);
        assert!(gather(&e).trailing_marker(&e, 0).is_none(), "concealed byte: no trailing cell");
        // true EOL (the newline byte, 5): fires with the landmark glyph.
        e.active_mut().marks.insert('y', 5);
        let sp = gather(&e).trailing_marker(&e, 0).expect("EOL mark fires");
        assert_eq!(sp.content.as_ref(), "·");
    }
    #[test]
    fn trailing_block_end_on_newline_yields_close_bracket() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 4, hidden: false });
        let sp = gather(&e).trailing_marker(&e, 0).expect("end-1 == newline byte fires");
        assert_eq!(sp.content.as_ref(), "]");
    }
    #[test]
    fn trailing_pending_at_empty_line_yields_open_bracket() {
        let mut e = Editor::new_from_text("para\n\nnext\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().pending_block_begin = Some(5); // the empty line
        let sp = gather(&e).trailing_marker(&e, 1).expect("empty-line anchor fires");
        assert_eq!(sp.content.as_ref(), "[");
    }
    /// (Codex plan-gate round 1, finding 2) The end-first tie-break WITHIN Boundary,
    /// actually pinned: a one-glyph block whose single byte IS the logical content end
    /// satisfies BOTH `mb.end - 1 == cend` and `mb.start == cend`; the `]` branch must
    /// win (spec §3.3 "a one-glyph-at-EOL block resolves end-first").
    #[test]
    fn trailing_one_glyph_block_at_eol_resolves_end_first() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        // block = the newline byte alone: [3, 4) → start == end-1 == cend == 3.
        e.active_mut().marked_block = Some(MarkedBlock { start: 3, end: 4, hidden: false });
        let sp = gather(&e).trailing_marker(&e, 0).expect("one-glyph EOL block fires");
        assert_eq!(sp.content.as_ref(), "]", "end-first tie-break within Boundary");
    }

    #[test]
    fn trailing_priority_one_cell_boundary_over_landmark() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().marked_block = Some(MarkedBlock { start: 0, end: 4, hidden: false });
        e.active_mut().marks.insert('m', 3); // same cend
        let sp = gather(&e).trailing_marker(&e, 0).expect("fires once");
        assert_eq!(sp.content.as_ref(), "]", "boundary outranks landmark");
    }

    // --- status helpers ---
    #[test]
    fn blk_direction_above_below_in_view() {
        let text = (0..50).map(|i| format!("line {i}\n")).collect::<String>();
        let mut e = Editor::new_from_text(&text, None, (40, 10));
        crate::derive::rebuild(&mut e);
        let b = MarkedBlock { start: 0, end: 6, hidden: false }; // "line 0"
        assert_eq!(blk_direction(&e, b), "", "in view at scroll 0");
        e.active_mut().view.scroll = 30;
        crate::derive::rebuild(&mut e);
        assert_eq!(blk_direction(&e, b), "↑", "scrolled past → above");
        let tail_start = text.rfind("line 49").unwrap();
        let b2 = MarkedBlock { start: tail_start, end: tail_start + 4, hidden: false };
        e.active_mut().view.scroll = 0;
        crate::derive::rebuild(&mut e);
        assert_eq!(blk_direction(&e, b2), "↓", "below the 10-row viewport");
    }
    #[test]
    fn marks_on_caret_line_lists_ids_in_order_and_clamps_stale() {
        let mut e = Editor::new_from_text("one\ntwo\n", None, (40, 10));
        e.active_mut().marks.insert('a', 5); // line 1
        e.active_mut().marks.insert('3', 4); // line 1
        e.active_mut().marks.insert('z', 0); // line 0
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(6);
        assert_eq!(marks_on_caret_line(&e), Some("MK 3,a".into()), "BTreeMap order: digits first");
        // stale mark PAST EOF (accepted undo drift): clamped, never a panic.
        e.active_mut().marks.insert('q', 999);
        let _ = marks_on_caret_line(&e); // must not panic; 'q' clamps to len (line 2)
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert_eq!(marks_on_caret_line(&e), Some("MK z".into()));
    }
}
```

**Verify:** `cargo test -p wordcartel block_paint` green; `cargo clippy --workspace
--all-targets` clean (dead-code: the production fns are `pub(crate)` and as-yet uncalled
outside tests — if clippy/rustc flags them, this is expected mid-effort ONLY if it
surfaces as a warning; the workspace denies warnings, so IF `dead_code` fires, silence is
NOT permitted — instead confirm: `pub(crate)` items used by the module's own tests do
not trip dead_code in practice; if the build disagrees, proceed directly to wiring the
ONE `gather` call of Task 4 step 2 into `gather_row_ctx` within THIS task and move the
corresponding test forward — record the deviation in the ledger).

**Commit:** `effort4 T3: block_paint seam — exclusive landmark classification + trailing-marker vectors` + trailers.

---

### Task 4: render.rs integration — paint lands, hub NET-SHRINKS

**RED.** Add to `render.rs`'s `mod tests` (baseline after T3: seam exists, render.rs
unchanged → all three fail as behavior, not compile):

```rust
    /// ④ B12: a lone ^KB anchor paints a boundary-faced cell (RED against the pre-④
    /// tree: pending_block_begin had zero render-side uses).
    #[test]
    fn pending_block_begin_paints_a_boundary_cell() {
        use ratatui::style::Modifier;
        let mut e = Editor::new_from_text("hello\n", None, (60, 6));
        e.active_mut().pending_block_begin = Some(2);
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 60, 6);
        let st = buf[(2u16, 0u16)].style();
        assert!(st.add_modifier.contains(Modifier::REVERSED | Modifier::BOLD),
            "anchor cell carries the boundary face");
        assert!(!buf[(0u16, 0u16)].style().add_modifier.contains(Modifier::REVERSED),
            "non-anchor cells untouched");
    }

    /// ④ B13: quiet interior, strong boundaries (the deliberate look change, spec §4.1).
    #[test]
    fn block_interior_is_quiet_and_boundaries_are_strong() {
        use ratatui::style::Modifier;
        let mut e = Editor::new_from_text("hello world\n", None, (60, 6));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 5, hidden: false });
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 60, 6);
        for x in [0u16, 4u16] {
            assert!(buf[(x, 0u16)].style().add_modifier.contains(Modifier::REVERSED | Modifier::BOLD),
                "col {x}: boundary cell (start / end-1)");
        }
        for x in 1u16..4 {
            let st = buf[(x, 0u16)].style();
            assert!(!st.add_modifier.contains(Modifier::REVERSED),
                "col {x}: interior is a tint, NOT reversed (pre-④ it was)");
            assert!(st.bg.is_some(), "col {x}: interior carries the tint bg");
        }
    }

    /// ④: mark presence cell + concealed-byte mark paints nothing anywhere (spec §3.3).
    #[test]
    fn landmark_cell_paints_and_concealed_mark_stays_invisible() {
        use ratatui::style::Modifier;
        let mut e = Editor::new_from_text("hello\n", None, (60, 6));
        e.active_mut().marks.insert('a', 1);
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 60, 6);
        let st = buf[(1u16, 0u16)].style();
        assert!(st.add_modifier.contains(Modifier::REVERSED | Modifier::ITALIC | Modifier::UNDERLINED),
            "mark cell carries the LandmarkGlyph face");

        // Concealed: LivePreview hides the `**` runs; a mark on the trailing `*` (byte 3)
        // paints NO cell — not in place (no glyph), not trailing (3 != cend 5).
        let mut e2 = Editor::new_from_text("**a**\n", None, (60, 6));
        e2.active_mut().marks.insert('x', 3);
        crate::derive::rebuild(&mut e2);
        let buf2 = render_to_buffer(&mut e2, 60, 6);
        for x in 0..10u16 {
            assert!(!buf2[(x, 0u16)].style().add_modifier.contains(Modifier::ITALIC),
                "col {x}: concealed-byte mark must paint nothing");
        }
    }
```

Run `cargo test -p wordcartel render` → the three fail. Red modes at the post-T3
baseline: pending/marks paint nothing (their asserts fail), and in
`block_interior_is_quiet_and_boundaries_are_strong` the BOUNDARY asserts fail (after
T2 the whole span is the quiet tint — no cell carries REVERSED+BOLD until this task's
classification lands; the interior asserts alone would already pass, the boundary ones
cannot). The two T2-migrated pins (`marked_block_paints_and_status_shows_blk`,
`hidden_block_status_reads_blk_hidden_and_not_painted`) stay green through this task —
they assert the strictly-interior col 2 and the no-tint hidden row, both invariant
under the T4 boundary split.

**GREEN.** Four edits in `render.rs` (locate by symbol):

1. `struct RowCtx` — replace the field `marked_block: Option<crate::editor::MarkedBlock>,`
   with `block_paint: crate::block_paint::BlockPaint,`.
2. `gather_row_ctx` — replace the three snapshot lines (`let marked_block = …; let
   block_hidden = …; let has_block = …;`) with:

```rust
    // Landmark snapshot (④): block + pending + marks, gathered once (block_paint seam).
    let block_paint = crate::block_paint::gather(editor);
```

   and in the `use_placed` expression replace `has_block` with
   `block_paint.wants_placed()`; in the `RowCtx { … }` literal replace `marked_block`
   with `block_paint`. Update the trailing comment ("a visible block forces the placed
   path" → "any landmark (block/pending/mark) forces the placed path").
3. `row_spans_placed` — signature gains `, last_row: bool` (after `row_dim: bool`).
   Replace the entire inline MarkedBlock arm (the `if let Some(b) = ctx.marked_block {…}`
   block, comment included) with:

```rust
        // Landmarks (④): ONE exclusive face per cell — boundary > pending > mark >
        // interior — composed BELOW Selection/Search/Lens/Diag, fold-safe as before.
        style = ctx.block_paint.patch_glyph(style, g_from, g_to, &editor.theme, editor.depth);
```

   After the final run flush (`if !run.is_empty() { … }`), before `spans`:

```rust
    if last_row {
        if let Some(sp) = ctx.block_paint.trailing_marker(editor, l) { spans.push(sp); }
    }
```

4. `paint_rows` — the one call site becomes
   `row_spans_placed(editor, &ctx, l, row_index, vr, map, row_dim, row_index + 1 == visual_rows.len())`.

**Verify:** `cargo test -p wordcartel` green (new + existing);
`cargo test -p wordcartel --test backlog` untouched-green; **budget check**:
`cargo test -p wordcartel --test module_budgets` green AND
`awk '/^mod tests/{print NR-1; exit}' wordcartel/src/render.rs` — this reproduces
`module_budgets::production_lines` (lines before `mod tests`; NOT raw `wc -l`) — prints
**< 899** (spec §6 requires strictly net-negative — expected ≈ 893). `cargo clippy --workspace
--all-targets` clean; `cargo build` + `cargo test --no-run` warning-free.

**Commit:** `effort4 T4: render — landmark paint via block_paint; render.rs net-shrinks` + trailers.

---

### Task 5: status segments — `BLK↑/↓`, `BLK…`, `MK <ids>`

**RED.** Add to `render_status.rs`'s `mod tests`. The first and third tests are
behavior-RED at baseline (no ④ segment exists); the second is an explicitly-labeled
green-at-intro PIN (see its doc comment):

```rust
    #[test]
    fn status_shows_pending_blk_ellipsis_and_direction() {
        let text = (0..50).map(|i| format!("line {i}\n")).collect::<String>();
        let mut e = Editor::new_from_text(&text, None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().pending_block_begin = Some(0);
        assert!(crate::render_status::status_left_text(&e).contains("BLK…"),
            "pending ^KB shows the mid-mark segment");
        e.active_mut().pending_block_begin = None;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 6, hidden: false });
        assert!(crate::render_status::status_left_text(&e).ends_with("· BLK"),
            "in view: plain BLK, no arrow");
        e.active_mut().view.scroll = 30;
        crate::derive::rebuild(&mut e);
        assert!(crate::render_status::status_left_text(&e).contains("BLK↑"),
            "scrolled below the block: BLK↑");
    }

    /// PIN, not red (Codex plan-gate round 1, finding 5): this asserts the EXACT
    /// legacy segment shape and is GREEN at introduction — its job is to stay green
    /// through the GREEN phase below, proving the ④ segments never leak onto a hidden
    /// block (no arrow, no pending, no MK — the status line ENDS at the legacy text).
    #[test]
    fn status_hidden_block_keeps_exact_legacy_segment() {
        let mut e = Editor::new_from_text("abc\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 3, hidden: true });
        let s = crate::render_status::status_left_text(&e);
        assert!(s.ends_with(" · BLK·hidden"),
            "hidden keeps the exact legacy tail — no arrow/pending/MK segment after it: {s}");
        assert!(!s.contains('↑') && !s.contains('↓') && !s.contains("BLK…") && !s.contains("MK "),
            "no ④ segment leaks onto a hidden block: {s}");
    }

    #[test]
    fn status_lists_marks_on_caret_line() {
        let mut e = Editor::new_from_text("one\ntwo\n", None, (40, 10));
        crate::derive::rebuild(&mut e);
        e.active_mut().marks.insert('a', 5);
        e.active_mut().marks.insert('3', 4);
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(6);
        assert!(crate::render_status::status_left_text(&e).contains("· MK 3,a"));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
        assert!(!crate::render_status::status_left_text(&e).contains("MK "),
            "no segment when the caret's line has no marks");
    }
```

**GREEN.** In `status_left_text`, replace the existing BLK `match` with:

```rust
    // BLK indicator (④ extends): `· BLK` gains a direction when fully off-screen
    // (`↑`/`↓`, line-granular); `· BLK·hidden` keeps its exact legacy form (no arrow
    // for an unpainted landmark); a pending ^KB shows `· BLK…` independently.
    match editor.active().marked_block {
        Some(b) if b.hidden => text.push_str(" · BLK·hidden"),
        Some(b) => {
            text.push_str(" · BLK");
            text.push_str(crate::block_paint::blk_direction(editor, b));
        }
        None => {}
    }
    if editor.active().pending_block_begin.is_some() {
        text.push_str(" · BLK…");
    }
    // Mark identity (④ sub-fork A): the caret line's mark names, BTreeMap order.
    if let Some(mk) = crate::block_paint::marks_on_caret_line(editor) {
        text.push_str(" · ");
        text.push_str(&mk);
    }
```

**Verify:** `cargo test -p wordcartel` green (incl. the render.rs status pins, which
match on substrings `BLK`/`BLK·hidden` and stay green); clippy clean.

**Commit:** `effort4 T5: status — BLK direction + BLK… pending + MK caret-line identity` + trailers.

---

### Task 6: C2 — `clear_mark` / `clear_marks`

**RED — two independent reds** (Codex plan-gate round 1, finding 3: the palette
invariant only checks rows that EXIST — a missing registration needs its own red).

First, add to `registry.rs`'s `mod tests` (this compiles at baseline — it references no
new function — and is RUNTIME-red until the GREEN phase's registry rows land; it stays
red even after the marks.rs bodies exist, catching a forgotten registration):

```rust
    /// ④ C2 (contract laws 1/3/4): the two clear commands are registered, palette-
    /// reachable (the palette enumerates the registry — presence IS reachability),
    /// menu-absent like every mark command, and read-only-safe (not `mutates`).
    #[test]
    fn clear_mark_commands_are_registered_palette_reachable() {
        let r = Registry::builtins();
        for (id, label) in [("clear_mark", "Clear Mark\u{2026}"), ("clear_marks", "Clear All Marks")] {
            let m = r.meta(CommandId(id)).unwrap_or_else(|| panic!("{id} must be registered"));
            assert_eq!(m.label, label);
            assert!(m.menu.is_none(), "{id}: palette-only, like every existing mark command");
            assert!(!m.mutates, "{id}: marks are buffer metadata — usable on read-only buffers");
        }
    }
```

(Accessors verified against source: `Registry::builtins()`, `Registry::meta(CommandId)
-> Option<&CommandMeta>`, and `CommandMeta`'s `pub label/menu/mutates` fields. Match the
test mod's existing imports — `use super::*` covers `Registry`/`CommandId`.)

Second, add to `marks.rs`'s `mod tests` (baseline: `clear_mark`/`clear_marks` and
`MarkPending::Clear` do not exist → red = compile error E0425/E0599 on this test mod):

```rust
    #[test]
    fn clear_mark_interactive_round_trip() {
        let mut e = Editor::new_from_text("0123456789\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(5);
        super::set_char_mark(&mut e, 'a');
        super::clear_mark(&mut e);
        assert_eq!(e.pending_mark, Some(MarkPending::Clear));
        super::resolve_pending(&mut e, 'a');
        assert!(e.active().marks.get(&'a').is_none(), "mark removed");
        assert_eq!(e.status_text(), "mark a cleared");
        assert!(!super::jump_char_mark(&mut e, 'a'), "jump now fails");
        // clearing an absent name reports, mutates nothing
        super::clear_mark(&mut e);
        super::resolve_pending(&mut e, 'q');
        assert_eq!(e.status_text(), "no mark q");
    }

    #[test]
    fn clear_marks_clears_all_and_counts() {
        let mut e = Editor::new_from_text("abcdef\n", None, (80, 24));
        super::set_char_mark(&mut e, '1');
        super::set_char_mark(&mut e, 'z');
        super::clear_marks(&mut e);
        assert!(e.active().marks.is_empty());
        assert_eq!(e.status_text(), "2 marks cleared");
        super::clear_marks(&mut e);
        assert_eq!(e.status_text(), "no marks");
    }
```

**GREEN** — sequenced so each red clears at its own step: steps 1-2 turn the marks.rs
tests green while `clear_mark_commands_are_registered_palette_reachable` STAYS red
(bodies exist, rows don't — run it to see the honest intermediate red); step 3 lands the
rows and clears it.

1. `editor.rs` — `pub enum MarkPending { Set, Jump, Clear }` (the only
   variant-exhaustive consumer is `marks::resolve_pending`; other references construct
   `Set` and keep compiling).
2. `marks.rs`:

```rust
pub fn clear_mark(editor: &mut Editor) { editor.pending_mark = Some(MarkPending::Clear); editor.set_status(crate::status::StatusKind::Info, "clear mark:"); }

/// Clear every mark in the active buffer (C2 — visible marks need a lifecycle).
pub fn clear_marks(editor: &mut Editor) {
    let n = editor.active().marks.len();
    editor.active_mut().marks.clear();
    editor.set_status(crate::status::StatusKind::Info,
        if n == 0 { "no marks".to_string() } else { format!("{n} marks cleared") });
}
```

   and in `resolve_pending`, after the `Jump` arm:

```rust
        Some(MarkPending::Clear) => {
            if editor.active_mut().marks.remove(&ch).is_some() {
                editor.set_status(crate::status::StatusKind::Info, format!("mark {ch} cleared"));
            } else {
                editor.set_status(crate::status::StatusKind::Info, format!("no mark {ch}"));
            }
        }
```

3. `registry.rs` — directly after the `jump_to_mark` row (`register`, NOT
   `register_mut`: marks are buffer metadata, usable on read-only buffers — the
   established `set_mark`/`set_bookmark_N` category):

```rust
        r.register("clear_mark",  "Clear Mark\u{2026}",  None, |c| { crate::marks::clear_mark(c.editor);  CommandResult::Handled });
        r.register("clear_marks", "Clear All Marks",     None, |c| { crate::marks::clear_marks(c.editor); CommandResult::Handled });
```

**Verify:** `cargo test -p wordcartel` green — including the EXISTING
palette-completeness invariant (law 3), which now covers the two rows with zero new
test code. Clippy clean.

**Commit:** `effort4 T6: C2 — clear_mark (interactive) + clear_marks; registry rows` + trailers.

---

### Task 7: probes, e2e journey, final verification (no merge)

**7a — e2e journey: a FINAL REGRESSION GATE, not red-first** (Codex plan-gate round 1,
finding 4). It runs after T1-T6, so it is EXPECTED TO PASS at introduction — its value
is cross-task synthesis (real command dispatch → paint → status → clear, in one flow)
and regression protection thereafter. The genuine behavior-red tests for these features
live in T4/T5/T6; do not dress this as red-first. In `e2e.rs`, the `Harness` idiom of
`journey_prose_lens_passive_paints_navigates_counts`:

```rust
/// ④ FINAL REGRESSION GATE (expected green once T1-T6 land): mark → see it; ^KB →
/// pending cell + BLK…; ^KK → quiet interior, strong boundaries + BLK; clear_marks
/// removes the painted mark. Real commands throughout.
#[test]
fn journey_landmarks_visible_and_clearable() {
    use crate::registry::{Ctx, CommandId};
    use ratatui::style::Modifier;
    let text = "alpha beta gamma\n";
    let mut h = Harness::new(text, None, (80, 24));
    let dispatch = |h: &mut Harness, id: &'static str| {
        let mut e = h.editor.borrow_mut();
        let clock = TestClock(h.now);
        let mut ctx = Ctx { editor: &mut e, clock: &clock, executor: &h.ex,
                            msg_tx: h.tx.clone(), fs: crate::test_support::test_fs() };
        h.reg.dispatch(CommandId(id), &mut ctx);
    };
    // 1) a bookmark paints at its cell
    h.editor.borrow_mut().active_mut().document.selection =
        wordcartel_core::selection::Selection::single(6); // 'b' of beta
    dispatch(&mut h, "set_bookmark_1");
    h.render();
    assert!(h.cell_modifiers(6, 0).contains(Modifier::ITALIC), "bookmark cell italic (LandmarkGlyph)");
    // 2) ^KB pending: cell + status
    h.editor.borrow_mut().active_mut().document.selection =
        wordcartel_core::selection::Selection::single(0);
    dispatch(&mut h, "block_begin");
    h.render();
    assert!(h.cell_modifiers(0, 0).contains(Modifier::BOLD), "pending anchor cell bold (boundary face)");
    assert!(h.screen_contains("BLK…"), "mid-mark status segment");
    // 3) ^KK completes: boundaries strong, interior quiet, BLK segment
    h.editor.borrow_mut().active_mut().document.selection =
        wordcartel_core::selection::Selection::single(5);
    dispatch(&mut h, "block_end");
    h.render();
    assert!(h.cell_modifiers(0, 0).contains(Modifier::REVERSED), "begin boundary");
    assert!(h.cell_modifiers(4, 0).contains(Modifier::REVERSED), "end boundary (b.end-1)");
    assert!(!h.cell_modifiers(2, 0).contains(Modifier::REVERSED), "interior quiet");
    assert!(h.screen_contains("· BLK"), "block status segment");
    // 4) clear_marks removes the painted bookmark
    dispatch(&mut h, "clear_marks");
    h.render();
    assert!(!h.cell_modifiers(6, 0).contains(Modifier::ITALIC), "mark gone after clear_marks");
}
```

If the harness lacks a `cell_modifiers(x, y) -> Modifier` helper (it has `cell_bg` —
verify by grep), add one beside it:

```rust
    fn cell_modifiers(&self, x: u16, y: u16) -> ratatui::style::Modifier {
        self.term.backend().buffer()[(x, y)].style().add_modifier
    }
```

**7b — residual probe 1 (clipping / B17 phantom flush row), TestBackend.** Add to
`render.rs` tests; the assertions are DISCOVERY assertions — run them, then pin whatever
the truth is (the spec §10.1 accepts clipping if real) and EDIT spec §10.1 to record the
outcome:

```rust
    /// ④ residual probe (spec §10.1): trailing marker on an exact-width row, and on a
    /// B17 phantom flush row (trailing space at the wrap margin). Pins REAL behavior.
    #[test]
    fn trailing_marker_exact_width_and_flush_row_probe() {
        // width 10 viewport; "0123456789" fills row 0 exactly; mark at EOL (byte 10).
        let mut e = Editor::new_from_text("0123456789\nnext\n", None, (10, 6));
        e.active_mut().marks.insert('a', 10);
        crate::derive::rebuild(&mut e);
        let buf = render_to_buffer(&mut e, 10, 6);
        // DISCOVERY: is the `·` visible on any row-0..1 cell, or clipped?
        let painted = (0..2u16).any(|r| row_string(&buf, r).contains('·'));
        // pin the observed value; update spec §10.1 with the verdict:
        //   clipped → accepted (status+jump remain the discovery path)
        //   visible → document WHERE (wrap continuation row)
        let _ = painted; // replace with an assert_eq! once observed

        // B17 phantom flush row: trailing space at the margin wraps the caret to a
        // flush continuation row — a mark at the line end rides the flush row.
        let mut e2 = Editor::new_from_text("0123 5678 \nx\n", None, (10, 6));
        let eol = "0123 5678 ".len(); // byte 10, the newline
        e2.active_mut().marks.insert('b', eol);
        crate::derive::rebuild(&mut e2);
        let _buf2 = render_to_buffer(&mut e2, 10, 6);
        // same discovery → pin + record.
    }
```

Executor note: this probe intentionally starts loose; the implementer REPLACES the
`let _ =` holes with concrete `assert…!` pins in the same task once the real behavior is
observed, and updates spec §10.1 in the same commit. A probe left un-pinned is a task
failure (the group③ "a verification step that cannot fail" lesson).

**7c — residual probe 2 (live legibility, the A21 lesson).** Via the `tui-interact`
skill, drive the REAL `wcartel` binary in a private tmux session: open a fixture with a
completed block, a pending anchor, and 2 marks (one at EOL); cycle
`tokyo-night → phosphor-green → terminal-plain → no-color` (theme picker); eyeball at
each: interior tint vs canvas, boundary reverse+bold+underline, landmark italic (some
terminals fake/drop italic), `reverse+dim` in mono. Record PASS/notes per theme in the
ledger AND spec §10.2. Advisory: a legibility miss is a FINDING for the human, not a
silent face change (faces are locked decisions).

**7d — final gates** (all must pass; quote outputs in the pre-merge report):
`cargo test --workspace`; `cargo build` + `cargo test --no-run` warning-free;
`cargo clippy --workspace --all-targets` clean; `cargo test -p wordcartel --test
module_budgets` + the awk render.rs count (< 899); `scripts/smoke/run.sh` — quote its
one-line summary VERBATIM (advisory, never blocking). Do NOT merge; do NOT touch
`backlog.toml` (backlog flips happen at merge per spec §9).

**Commit:** `effort4 T7: e2e journey + residual probes (clipping/B17 pinned, live legibility recorded)` + trailers.

---

## Self-Review (performed at plan time)

- Every red test names its baseline and its failure mode (compile-red for new APIs,
  behavior-red for new paint/status — verified possible: pending/marks paint nothing at
  baseline; interior IS reversed at baseline, so the not-reversed assert genuinely
  fails).
- All signatures re-verified against `6067ad5`: `MarkedBlock`/`pending_block_begin`/
  `marks` fields; `overlaps` is `pub(crate)` in render.rs; `line_layouts:
  BTreeMap<usize, (Vec<VisualRow>, ColMap)>` (so `last_key_value()` is available);
  `ventilate::resolve -> Option<Resolved { last_line, .. }>`; `derive::line_start`
  clamps; `modface`/`m` has NO dim param (hence the mono interior literal);
  `ThemeFaces` is the struct's real name; default `RenderMode` is `LivePreview`
  (editor.rs `View` construction) so concealment vectors run on a fresh Editor;
  `Harness::new` + `reg.dispatch(CommandId(..), &mut Ctx{..})` + `screen_contains` +
  `cell_bg` are the e2e idioms (`cell_modifiers` may need adding — the plan includes it).
- `Span.content` is a `Cow<str>` — the trailing tests compare via `.as_ref()`.
- Budget math: −3 (gather) −8+1 (arm) +3 (trailing) +1 (param) ≈ −6 → ~893; the awk
  check makes it a hard verification, not an estimate.
- Undo-drift clamp: `marks_on_caret_line` is the ONLY new buffer-indexed read of a mark
  position; it clamps (`pos.min(buf.len())`) with a red test. Paint-path mark reads are
  pure integer compares — no clamp needed.
- Absolute-column asserts (T4 render tests, T7 journey) are sound: `view_opts.measure`
  defaults FALSE (`config.rs` `ViewOptions` default: `measure: false, wrap_column: 72`),
  so `text_geometry` yields `text_left = 0` at every test width — screen col == byte col
  on an unwrapped ASCII row (the S8 paint-test idiom).
- Mid-effort dead-code watch (T3): `pub(crate)` seam items are uncalled by production
  until T4; if `cargo build -p wordcartel` raises `dead_code` at T3's commit point, the
  T3 verify section's contingency (wire the single `gather` call early) applies —
  recorded there, not discovered at gate time.
- Intermediate-green audit (round 2): after EACH commit, `cargo test --workspace` is
  green — T1 additive; T2 migrates every pin of the old block look IN the face-change
  commit (theme mono test + `ThemeFaces` fixture + the two render.rs tests, step 8;
  census found no others — the render.rs face-distinctness battery compares whole
  `Face`s with per-theme-distinct bgs, and e2e/compose/theme_picker/base16 carry no
  MarkedBlock styling asserts); T3 is a new uncalled module + its own tests; T4's
  migrated pins assert the T4-invariant interior cell, so they survive the boundary
  split; T5/T6 are additive with their own reds cleared in-task.

## Codex plan-gate fold log

- **Round 1 (2026-07-24, NO-GO → folded):** 4 Important + 2 Minor, all
  TDD-coverage/test-tightening; no design change.
  1. (Important) T2: `element_from_key` had no red for the two NEW keys — the existing
     `element_from_key_maps_snake_case_names` checks only current keys. Added edit 4 to
     T2 RED (both keys asserted; runtime-red if the arms are forgotten).
  2. (Important) T3: the one-glyph-at-EOL `]`-first tie-break was claimed, not pinned.
     Added `trailing_one_glyph_block_at_eol_resolves_end_first` (block `[3,4)` on
     `"abc\n"`: `start == end-1 == cend == 3` → `"]"`).
  3. (Important) T6: a MISSING registry row escaped every test (the palette invariant
     covers only rows that exist). Added
     `clear_mark_commands_are_registered_palette_reachable` in `registry.rs` tests —
     compiles at baseline, RUNTIME-red until the rows land (stays red after the marks.rs
     bodies exist); GREEN re-sequenced into 3 steps to exercise that intermediate red.
     Accessors verified: `Registry::builtins()`, `meta(CommandId) ->
     Option<&CommandMeta>`, pub `label`/`menu`/`mutates`.
  4. (Important) T7: the e2e journey ran after T1-T6 and could not be red — relabeled
     explicitly as a FINAL REGRESSION GATE (expected green at introduction); the
     genuine reds stay in T4/T5/T6. Probes 7b/7c unchanged.
  5. (Minor) T5: hidden-block status test tightened from `contains("BLK·hidden")` to
     `ends_with(" · BLK·hidden")` + no-④-segment-leak asserts, and honestly labeled a
     green-at-intro PIN (it guards the legacy form; it cannot be red).
  6. (Minor) Global-constraints prose said `MenuCategory::None` (nonexistent) — menu
     absence is `menu: None` on `CommandMeta.menu: Option<MenuCategory>`. Fixed; the
     T6 code snippet already used `None` correctly.
- **Round 2 (2026-07-24, NO-GO → folded; all round-1 folds confirmed resolved):**
  1. (Important) T2 broke the intermediate-green invariant: dropping REVERSED from the
     `MarkedBlock` interior fails the existing
     `marked_block_paints_and_status_shows_blk` (its `row_has_highlight` helper sees
     only Yellow-bg/REVERSED), and the strong boundary cues don't land until T4 — so
     `cargo test` would be red between T2 and T4. Fix: T2 gains GREEN step 8 migrating
     BOTH old-look render.rs pins in the same commit — direct-style asserts on the new
     interior form (tint bg present + NOT reversed at a strictly-interior cell, col 2,
     which stays `Interior` through T4's boundary split; hidden test asserts
     no-tint-anywhere instead of the now-blind `row_has_highlight`). The helper is NOT
     widened (it serves search/selection with exactly its cues); the tests stay
     meaningful (dropping interior paint breaks the tint-bg equality). Census: no other
     test pins the old look (theme mono test + `ThemeFaces` fixture already migrate in
     T2; the render.rs face-distinctness battery compares whole `Face`s and survives;
     e2e/compose/theme_picker/base16 grep-clean). T4's red-mode description updated to
     match (boundary asserts are its red; the migrated pins stay green).
  2. (Minor) Every 899-baseline citation now says "production lines
     (`module_budgets::production_lines`, lines before `mod tests`)" — raw `wc -l` is
     ~4286; the awk check is annotated as reproducing the same count.
