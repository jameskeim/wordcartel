# SRC-HI Syntax Highlighting — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Make `SourceHighlighted` (SRC-HI) render raw markdown with every construct colored in the current theme's element faces (uniform per-construct coloring — inline delimiters, block prefixes, and content all take their construct's existing face), while `SourcePlain` stays monochrome and `LivePreview` is untouched.

**Architecture:** Replace the two-value `is_active: bool` that flows into the layout/style engine with a three-way `LineRender { Concealed, RawPlain, RawStyled }` descriptor. `Concealed` and `RawPlain` reproduce today's two behaviours byte-for-byte; `RawStyled` (new) is "reveal all markers + style every construct." Color is a pure visual overlay: SRC-HI conceals nothing, so its geometry (ColMap/cursor/wrap/fold) is identical to `SourcePlain`.

**Tech Stack:** `wordcartel-core` (pure: `md_parse`, `layout`, `style`) + `wordcartel` shell (`derive`, `nav`, `render`, ratatui 0.30).

**Source spec:** `docs/superpowers/specs/2026-07-07-wordcartel-srchi-syntax-highlight-design.md` (Codex spec gate GO).

## Global Constraints

- **LivePreview and SourcePlain must be behaviourally identical to today** — only SRC-HI changes. `Concealed` == current `is_active=false`; `RawPlain` == current `is_active=true` (used by both the LivePreview *active* line and every SourcePlain line).
- **Geometry ≡ SourcePlain for SRC-HI.** SRC-HI conceals nothing; no cursor/wrap/fold/ColMap change is permitted. Color must not move the caret. (Verified: `layout` derives geometry from visible graphemes/widths only; `styles` populate only `VG.style`/`StyledSeg.style` — `layout.rs:259,388`.)
- **Reuse existing theme faces — no new `SemanticElement`.** Delimiters take their construct's existing face (Strong/Heading/Code/Link/…), so SRC-HI tracks the active theme by construction.
- **Exhaustive matches** on `RenderMode` and `LineRender` — no catch-all `_`.
- **House style / GATEs:** `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean; `cargo build`/`--no-run` warning-free for touched crates; no `cargo fmt`; em-dash prose comments; smoke mandatory-run. Doc-comment new public items.

---

## File Structure

- `wordcartel-core/src/style.rs` — add `pub enum LineRender`.
- `wordcartel-core/src/md_parse.rs` — `analyze` takes `LineRender`; add the `RawStyled` branch (no conceal + whole-construct delimiter styling).
- `wordcartel-core/src/layout.rs` — `layout`/`visible_width`/`visible_source` take `LineRender`; change per-grapheme style resolution to **last-match-wins**; `ColMap.is_active` semantics preserved as "raw (not concealed)".
- `wordcartel/src/derive.rs` — map `RenderMode` + per-line active-ness → `LineRender` (shared helper); `LayoutKey` carries the mode.
- `wordcartel/src/nav.rs` — pass `LineRender` via the shared helper (geometry-only; conceal must match render).
- `wordcartel/src/render.rs` — flip the color gate from `source_mode` (`mode != LivePreview`) to `plain_source` (`mode == SourcePlain`) in BOTH the segs and placed paint paths.

**Task order:** Task 1 (core: descriptor + analyze + layout) → Task 2 (shell wiring: derive/LayoutKey/nav) → Task 3 (shell render gate + whole-effort tests).

---

## Task 1: Core — `LineRender` descriptor, `analyze` RawStyled branch, last-match-wins style resolution

**Files:**
- Modify: `wordcartel-core/src/style.rs` (add enum)
- Modify: `wordcartel-core/src/md_parse.rs:11` (`analyze` signature + branches)
- Modify: `wordcartel-core/src/layout.rs:240` (`layout`), `:556` (`visible_width`), `:570` (`visible_source`) signatures; `:259-264` style resolution; `:286`/`:416` `is_active` uses
- Test: inline `#[cfg(test)]` in `md_parse.rs` + `layout.rs`

**Interfaces:**
- Produces: `pub enum LineRender { Concealed, RawPlain, RawStyled }` (Copy, Eq); `analyze(line, role, render: LineRender) -> LineAnalysis`; `layout(line, role, render: LineRender, vw, heading_prefix)`; `visible_width(line, role, render)`; `visible_source(line, role, render)`.
- Consumes: nothing new.

- [ ] **Step 1: Write failing tests.**

`md_parse.rs` tests:
```rust
#[test]
fn raw_styled_reveals_all_markers_and_styles_delimiters_and_content() {
    // "**bold**": RawStyled reveals every byte (no conceal) AND styles the whole
    // construct (delimiters + content) Strong.
    let a = analyze("**bold**", BlockRole::Paragraph, LineRender::RawStyled);
    assert!(a.runs.iter().all(|r| r.visible), "RawStyled conceals nothing");
    // every byte of "**bold**" resolves to Strong (delimiters included)
    for b in 0.."**bold**".len() {
        let s = a.styles.iter().filter(|s| s.src.contains(&b)).last().map(|s| s.style);
        assert_eq!(s, Some(Style::Strong), "byte {b} of **bold** must be Strong");
    }
}

#[test]
fn raw_styled_nested_delimiters_take_position_style() {
    // "**_x_**": the outer ** = Strong, the inner _ and x = Strong+Em (StrongEmphasis).
    let a = analyze("**_x_**", BlockRole::Paragraph, LineRender::RawStyled);
    let at = |b: usize| a.styles.iter().filter(|s| s.src.contains(&b)).last().map(|s| s.style);
    assert_eq!(at(0), Some(Style::Strong), "opening ** is Strong");
    let ux = "**_x_**".find("_x_").unwrap();
    assert_eq!(at(ux), Some(Style::StrongEmphasis), "inner _ is Strong+Em");        // '_'
    assert_eq!(at(ux + 1), Some(Style::StrongEmphasis), "x is Strong+Em");          // 'x'
}

#[test]
fn concealed_and_rawplain_unchanged() {
    // Concealed == old is_active=false; RawPlain == old is_active=true.
    let c = analyze("**bold**", BlockRole::Paragraph, LineRender::Concealed);
    assert!(c.runs.iter().any(|r| !r.visible), "Concealed still hides the ** markers");
    let p = analyze("**bold**", BlockRole::Paragraph, LineRender::RawPlain);
    assert!(p.runs.iter().all(|r| r.visible) && p.styles.is_empty(), "RawPlain = raw, no styles");
}
```
(`Style::StrongEmphasis` — use whatever `current_style(strong, em, …)` returns for strong+em; read `style.rs`'s `Style` variants and `current_style` at `md_parse.rs` and name the real variant. If the combined variant is named differently, adjust the assertion to the real name.)

`layout.rs` test (last-match-wins is a no-op for the existing non-overlapping spans):
```rust
#[test]
fn concealed_layout_style_unchanged_under_last_match() {
    // A single Strong content span still styles "bold" Strong (resolution change is a no-op here).
    let (rows, _) = layout("**bold**", BlockRole::Paragraph, LineRender::Concealed, 40, false);
    let joined: String = rows.iter().flat_map(|r| r.segs.iter()).map(|s| s.text.clone()).collect();
    assert_eq!(joined, "bold", "Concealed still conceals the markers");
}
```

- [ ] **Step 2: Run — confirm FAIL** (`LineRender` undefined).

- [ ] **Step 3: Implement.**

`style.rs`:
```rust
/// How one logical line is rendered into visual rows. Replaces the old
/// `is_active: bool`. `Concealed` hides markdown markers and styles content
/// (LivePreview inactive lines). `RawPlain` shows raw source with no styles
/// (the LivePreview active/caret line and all SourcePlain lines). `RawStyled`
/// shows raw source with every construct — delimiters, block prefixes, and
/// content — styled in its element face (SourceHighlighted). Concealment
/// (hence geometry) is identical for `RawPlain` and `RawStyled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineRender { Concealed, RawPlain, RawStyled }
```

`md_parse.rs::analyze` — change the signature and the two gates. Replace the `is_active` early-return guard and the conceal application:
```rust
pub fn analyze(line: &str, role: BlockRole, render: LineRender) -> LineAnalysis {
    // RawPlain: show raw source, no styles (the old is_active=true path).
    if render == LineRender::RawPlain || line.is_empty() {
        return LineAnalysis {
            runs: vec![Run { src: 0..line.len(), visible: true }],
            styles: vec![],
            role,
            prefix_glyph: None,
        };
    }
    let raw_styled = render == LineRender::RawStyled; // reveal all + style delimiters
    // … existing parser setup unchanged …
```
In the event loop, for `RawStyled` push a whole-span style at each inline `Start` (so delimiters get the construct's colour). Keep the existing `conceal.push` for the `Concealed` case; for `RawStyled` we do NOT conceal but DO style the whole span:
```rust
            Event::Start(Tag::Strong) => {
                strong += 1;
                if raw_styled { styles.push(StyleSpan { src: range, style: current_style(strong, em, strike, link) }); }
                else { conceal.push(range); }
            }
            Event::End(TagEnd::Strong) => { strong = strong.saturating_sub(1); }
            // …identical shape for Emphasis / Strikethrough / Link: increment, then
            //   raw_styled ? push whole-span style : conceal.push(range) …
```
(Note the counter is incremented BEFORE `current_style` so the whole span carries its own construct's style; nesting is handled by last-match-wins in `layout`. The `End` arms are unchanged.)

For `Event::Code`: in `raw_styled`, style the WHOLE range (backticks + inner) as `Code` and do not conceal the fence:
```rust
            Event::Code(_) => {
                if raw_styled {
                    styles.push(StyleSpan { src: range.clone(), style: Style::Code });
                } else {
                    // existing conceal-fence + style-inner logic, unchanged
                }
            }
```
For `Event::Text`: unchanged (content styling — still pushed; with last-match-wins it wins for content). For `Event::InlineHtml` comment: unchanged. Then gate the conceal application so `RawStyled` reveals everything:
```rust
    // Apply conceal only when NOT raw (RawStyled reveals all markers).
    if !raw_styled {
        for r in conceal { for b in r { if b < n { visible[b] = false; } } }
        for r in reveal  { for b in r { if b < n { visible[b] = true;  } } }
        // escapes + block-prefix conceal stay inside this block (raw shows them too)
        // … existing escape loop …
        prefix_glyph = apply_block_prefix_conceal(&mut visible, line, &role);
    }
```
(Restructure so `prefix_glyph`/escape only run for the concealed path; for `RawStyled` every byte stays visible and `prefix_glyph = None`. Block-prefix COLOUR still comes from `role_element` at render time — Task 3 — so `## ` gets the heading colour even though it's an un-concealed run here.)

`layout.rs` — change per-grapheme style resolution from first-match to **last-match-wins** (`layout.rs:259-264`):
```rust
            let style = analysis
                .styles
                .iter()
                .filter(|s| s.src.contains(&byte_start))
                .last()                    // last-match-wins: no-op for non-overlapping Concealed spans
                .map(|s| s.style)
                .unwrap_or(Style::Plain);
```
Thread `LineRender` through `layout`/`visible_width`/`visible_source` signatures (replace `is_active: bool`), pass `render` to `analyze`, and update the two internal `is_active` uses:
- `ColMap.is_active` field (`layout.rs:72`) + its set at `layout.rs:416`: keep the field but set it to `matches!(render, LineRender::RawPlain | LineRender::RawStyled)` (its meaning is "raw / not concealed" — preserved for any downstream reader).
- The heading-glyph-placeholder gate `layout.rs:286` (`… && !is_active && …`): becomes `… && render == LineRender::Concealed && …` (the shade placeholder is a concealed-view affordance only).

- [ ] **Step 4: Run — PASS.** `cargo test -p wordcartel-core` green (existing `md_parse`/`layout` tests updated to pass `LineRender::Concealed`/`RawPlain` instead of `false`/`true` — mechanical).

- [ ] **Step 5: Commit** — `feat(core): LineRender descriptor + RawStyled analyze (styled raw markdown) + last-match-wins`.

---

## Task 2: Shell wiring — derive mapping, `LayoutKey` mode-awareness, nav

**Files:**
- Modify: `wordcartel/src/derive.rs` (`LayoutKey` `:10-18`; mapping at `:222`/`:259-267`; `source_mode` field)
- Modify: `wordcartel/src/nav.rs` (`:64-67`, `:148`)
- Test: inline in `derive.rs`

**Interfaces:**
- Consumes: `LineRender` (Task 1).
- Produces: a shared `pub(crate) fn line_render_for(mode: RenderMode, is_active_line: bool) -> LineRender`; `LayoutKey` carries `mode: RenderMode` (replacing `source_mode: bool`).

- [ ] **Step 1: Write failing test** — cache invalidates on SH↔SP.
```rust
#[test]
fn layout_cache_distinguishes_srchi_from_source() {
    let mut e = Editor::new_from_text("**bold**\n", None, (40, 6));
    e.active_mut().view.mode = crate::editor::RenderMode::SourceHighlighted;
    crate::derive::rebuild(&mut e);
    let sh_segs: String = e.active().view.line_layouts[&0].0.iter()
        .flat_map(|r| r.segs.iter()).map(|s| format!("{:?}", s.style)).collect();
    e.active_mut().view.mode = crate::editor::RenderMode::SourcePlain;
    crate::derive::rebuild(&mut e);
    let sp_segs: String = e.active().view.line_layouts[&0].0.iter()
        .flat_map(|r| r.segs.iter()).map(|s| format!("{:?}", s.style)).collect();
    assert_ne!(sh_segs, sp_segs, "SH must re-layout with styles; SP stays Plain (cache must not alias)");
}
```

- [ ] **Step 2: Run — FAIL** (SH and SP alias the cache via `source_mode: bool`, and/or styles identical pre-Task-3-wiring). If Task 1's core change already differentiates the segs' styles, this still fails on the cache-key aliasing until fixed.

- [ ] **Step 3: Implement.**

`derive.rs` — the shared mapping + use it:
```rust
/// Map the view's render mode + whether this is the caret line → LineRender.
/// LivePreview conceals inactive lines and shows the active line raw+plain;
/// SourceHighlighted styles every line raw; SourcePlain shows every line raw+plain.
pub(crate) fn line_render_for(mode: crate::editor::RenderMode, is_active_line: bool)
    -> wordcartel_core::style::LineRender
{
    use crate::editor::RenderMode::*;
    use wordcartel_core::style::LineRender::*;
    match mode {
        LivePreview      => if is_active_line { RawPlain } else { Concealed },
        SourceHighlighted => RawStyled,
        SourcePlain      => RawPlain,
    }
}
```
Replace the layout call at `derive.rs:259-267`:
```rust
        let render = line_render_for(b_mode, l == active_line); // b_mode read alongside active_line
        let (rows, map) = layout::layout(&text, role, render, vp_width, editor.theme.heading_level_glyph);
```
(capture `b.view.mode` into `b_mode` in the same borrow block that reads `active_line`, `derive.rs:208-224`; drop the now-unused `source_mode` local.)

`LayoutKey` (`derive.rs:10-18`, built `:234-244`): replace `pub source_mode: bool` with `pub mode: crate::editor::RenderMode` and set `mode: b.view.mode` in the constructor. (`LayoutKey` is compared with `==` at `derive.rs:245` — `derive(PartialEq, Eq)`, not hashed — and `RenderMode` already derives `Clone, Copy, PartialEq, Eq, Debug` (`editor.rs:44`), so NO new derive is needed on either.)

`nav.rs` (`:64-67`, `:148`) — use the shared mapping so nav's concealment matches render exactly (geometry alignment):
```rust
    // :64-67
    let render = crate::derive::line_render_for(editor.active().view.mode, l == caret_line(editor));
    let (_rows, map) = layout::layout(&text, role, render, vp_width, editor.theme.heading_level_glyph);
    // :148 (always-raw caret op) → LineRender::RawPlain
    let (_rows, map) = layout::layout(&text, role, wordcartel_core::style::LineRender::RawPlain, vp_width, editor.theme.heading_level_glyph);
```
(nav discards `rows`/styles and keeps `map`; RawStyled vs RawPlain give the identical ColMap since both conceal nothing, so geometry is exact.)

- [ ] **Step 4: Run — PASS.** Full `cargo test -p wordcartel --lib` green.

- [ ] **Step 5: Commit** — `feat(derive): LineRender mapping + mode-aware LayoutKey; nav uses shared mapping`.

---

## Task 3: Shell render — colour gate for both paint paths + whole-effort tests

**Files:**
- Modify: `wordcartel/src/render.rs` (`:607` gate; segs path `:676-692`; placed path `:746-763`; the `:1658` test)
- Test: inline in `render.rs`

**Interfaces:**
- Consumes: `RenderMode`.

**Design:** SourceHighlighted's raw-but-styled segs are already produced by layout (Task 1/2), so LivePreview and SourceHighlighted share the *coloured* render branch; only SourcePlain stays `[SE::Text]`. The gate flips from `source_mode` (`mode != LivePreview`) to `plain_source` (`mode == SourcePlain`).

- [ ] **Step 1: Write failing tests.**
```rust
#[test]
fn srchi_colors_delimiters_and_content_source_stays_plain() {
    // Render "**bold**" + "# H" in SH vs SP on a colored theme; SH applies Strong/Heading
    // faces to the RAW text (delimiters included), SP is monochrome base.
    // (Drive render() against a TestBackend; compare the fg of the '*' cell and the '#' cell
    //  between SH and SP — SH != SP, SP == base_fg. Mirror the TestBackend setup in
    //  render.rs tests, e.g. source_mode_no_heading_fg_live_preview_has_heading_fg.)
    // Assert: in SH the first '*' cell fg == the Strong face fg (not base); in SP it == base.
    // Assert: in SH the '#' cell fg == the heading-level face fg; in SP == base.
}

#[test]
fn live_preview_and_source_plain_render_unchanged() {
    // Pin: LivePreview conceals markers + colors content; SourcePlain shows raw + base only.
    // (Reuse/duplicate the existing LP + SP render assertions; they must be byte-identical.)
}
```
(Fill the `TestBackend` bodies concretely against `render::render` — read the neighbouring render tests for the exact backend/cell-fg helpers.)

- [ ] **Step 2: Run — FAIL** (SH currently paints `[SE::Text]`, monochrome).

- [ ] **Step 3: Implement.** In `render.rs`, replace the gate (`:607`):
```rust
    // SourcePlain is the only mode with no semantic colour; LivePreview and
    // SourceHighlighted both paint role + inline styles (SH's raw markers carry
    // their construct's style from layout, so they colour too).
    let plain_source = editor.active().view.mode == crate::editor::RenderMode::SourcePlain;
```
In BOTH paint branches — the segs path (`:676-692`) and the placed path (`:746-763`) — replace every `if source_mode` with `if plain_source` (unchanged structure; the `row_dim` FocusDim sub-branch keeps composing `SE::FocusDim` OVER the semantic stack for the non-plain case, which now includes SH). No other change: the coloured branch already composes `[SE::Text, role_element(vr.role), style_element(<style>)]`, which now applies to SH's styled raw segs.

Update the `:1658` test `source_mode_no_heading_fg_live_preview_has_heading_fg`: it asserts "source mode → no heading fg." That is now true only for **SourcePlain**; **SourceHighlighted** DOES get the heading fg. Set the test's source case to `SourcePlain` (keeps passing) and add a `SourceHighlighted` case asserting the heading fg IS present (the new behaviour).

- [ ] **Step 4: Run — PASS.** Then the whole-effort checks: `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean; `cargo build --no-run` warning-free; run `scripts/smoke/run.sh` (quote the summary).

- [ ] **Step 5: Commit** — `feat(render): SRC-HI colours raw markdown (plain_source gate, both paint paths)`.

---

## Testing & gates (whole-effort)

- Per-task TDD tests above, plus these cross-cutting pins confirmed before final review:
  - **SH ≠ SP** (the missing coverage that hid B4): delimiters + content + heading all themed in SH, monochrome in SP.
  - **Geometry SH ≡ SP:** same ColMap / cursor stop / wrap for a fixture with concealable markers (a caret-column probe in SH must equal SP).
  - **LivePreview + SourcePlain byte-identical to pre-effort.**
  - **Cache invalidation** cycling SH↔SP (Task 2).
- GATEs: `cargo test -p wordcartel-core -p wordcartel` green; `cargo clippy --workspace --all-targets` clean; build/`--no-run` warning-free; smoke mandatory-run.
- Pipeline gates: Codex plan review (loop clean) → subagent execution → Codex pre-merge + Fable whole-branch → merge.

## Notes for the executor

- `current_style(strong, em, strike, link)` and the `Style` variant names are in `md_parse.rs`/`style.rs` — use the REAL names in the RawStyled pushes and tests (the plan's `Style::StrongEmphasis` is illustrative).
- `LayoutKey` is compared via `==` (`derive(PartialEq, Eq)`, NOT hashed — `derive.rs:245`), so `mode: RenderMode` needs only `PartialEq, Eq`, which `RenderMode` already derives (`editor.rs:44`: Clone, Copy, PartialEq, Eq, Debug). **No derive change needed.**
- `visible_width`/`visible_source` are used only by core layout + tests (per the grounding) — the signature change is contained; update their test call sites (`layout.rs:1024,1068,1155`, `md_parse.rs:393+`) from the bool to the descriptor.
