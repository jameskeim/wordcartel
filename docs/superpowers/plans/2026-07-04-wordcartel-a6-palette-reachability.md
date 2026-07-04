# A6 Overlay List Reachability Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** windowed scrolling for the four diseased overlays (palette incl. buffer-switcher, outline, theme picker, file browser) — full-list keyboard reach (arrows/PgUp/PgDn/Home/End), wheel-moves-selection, a `{selected+1}/{total}` border indicator — killing the invisible-Enter-dispatch hazards (palette + file browser) and the theme picker's invisible-preview variant. **The invariant: the selection is ALWAYS visible.**

**Architecture:** a new shared `list_window.rs` (pure `list_h_for` + `keep_visible`); `scroll_top` on the four structs; key/mouse layers keep the window following the SELECTION; **render self-heals** (a `keep_visible` pre-pass per painter with the live frame's `list_h` — user decision A) so the invariant survives resize and any future geometry change; the file browser's descend path explicitly resets its window.

**Tech Stack:** Rust, ratatui 0.30 (`Block::title_bottom` + `Line::right_aligned` for the indicator), the e2e `Harness`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-07-04-wordcartel-a6-palette-reachability-design.md` (Codex ×4 + Fable5; user decisions: all-four scope, wheel-moves-selection, render self-heal). **SATURATING arithmetic everywhere** (empty row sets are reachable — the M2 overflow class); the two asymmetric layers (selection-following vs geometry-healing) are design, not redundancy.
- `cargo test -p wordcartel-core -p wordcartel` green; warning-free builds; **clippy deny gate LIVE**; NO `cargo fmt`; house em-dash `—`.
- Never weaken a test; the hazard pins' red states are specified per test below.
- **Pre-merge report:** `scripts/smoke/run.sh` summary quoted verbatim (advisory); a live tmux sanity (open the palette, press End — the last command visible + the indicator shows `110/110`-ish).
- Trailers on every commit, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

### Task 1: `list_window.rs` + the full palette fix

**Files:**
- Create: `wordcartel/src/list_window.rs` (+ `mod list_window;` in main.rs/lib wiring beside the other modules).
- Modify: `wordcartel/src/palette.rs` (field + rebuild tail), `wordcartel/src/app.rs` (palette key arms :1218-1292), `wordcartel/src/render.rs` (:150-174 helpers, :720-777 painter), `wordcartel/src/mouse.rs` (:122-145 wheel + the click-test), `wordcartel/src/e2e.rs` (the journey).

**Interfaces produced:** `list_window::{list_h_for, keep_visible}`; `Palette.scroll_top`; the windowed-painter pattern Task 2 replicates.

- [ ] **Step 1: the shared module** (`wordcartel/src/list_window.rs`, complete):
```rust
//! Windowed-list helpers for overlay lists (palette, outline, theme picker,
//! file browser — A6). Two layers keep the "selection is always visible"
//! invariant: key/mouse handlers call `keep_visible` after every selection
//! change (the window follows the SELECTION); each render painter calls it
//! again with the live frame's `list_h` (the window respects the GEOMETRY —
//! survives resize without an event hook).

/// Visible row budget for a windowed overlay list — the single source of the
/// min(rows, 15, h-4) computation (previously duplicated by
/// `palette_overlay_rect` and `palette_row_at`).
pub(crate) fn list_h_for(row_count: usize, area_h: u16) -> usize {
    row_count.min(15).min(area_h.saturating_sub(4) as usize)
}

/// Slide the window so `selected` is visible: on exit (for `list_h > 0`),
/// `selected ∈ [scroll_top, scroll_top + list_h)` and
/// `scroll_top <= row_count.saturating_sub(list_h)` (no over-scroll after a
/// shrink). `list_h == 0` (degenerate terminal) resets the window to 0.
pub(crate) fn keep_visible(selected: usize, row_count: usize, list_h: usize, scroll_top: &mut usize) {
    if list_h == 0 {
        *scroll_top = 0;
        return;
    }
    if selected < *scroll_top {
        *scroll_top = selected;
    } else if selected >= *scroll_top + list_h {
        *scroll_top = selected + 1 - list_h;
    }
    *scroll_top = (*scroll_top).min(row_count.saturating_sub(list_h));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_h_for_takes_the_three_way_min() {
        assert_eq!(list_h_for(110, 24), 15, "cap wins");
        assert_eq!(list_h_for(3, 24), 3, "row count wins");
        assert_eq!(list_h_for(110, 10), 6, "terminal wins (h-4)");
        assert_eq!(list_h_for(110, 4), 0, "degenerate");
    }

    #[test]
    fn keep_visible_window_follows_selection() {
        let mut top = 0;
        keep_visible(20, 110, 15, &mut top);
        assert_eq!(top, 6, "below the window: selected becomes the last visible row");
        keep_visible(3, 110, 15, &mut top);
        assert_eq!(top, 3, "above the window: selected becomes the first visible row");
        keep_visible(10, 110, 15, &mut top);
        assert_eq!(top, 3, "inside the window: no movement");
        keep_visible(109, 110, 15, &mut top);
        assert_eq!(top, 95, "End lands the window on the tail");
    }

    #[test]
    fn keep_visible_reclamps_after_shrink_and_degenerate() {
        let mut top = 95;
        keep_visible(2, 3, 15, &mut top);
        assert_eq!(top, 0, "filter shrink: over-scroll clamped away");
        let mut top = 5;
        keep_visible(50, 110, 0, &mut top);
        assert_eq!(top, 0, "list_h == 0 resets the window");
        let mut top = 0;
        keep_visible(0, 0, 15, &mut top);
        assert_eq!(top, 0, "empty rows: no movement, no underflow");
    }
}
```

- [ ] **Step 2: `Palette.scroll_top`.** In palette.rs (:19-28), add to the struct (it derives
  `Default` — no ctor changes):
```rust
    /// First visible row of the windowed list (A6). Maintained by keep_visible.
    pub scroll_top: usize,
```
  And the `rebuild_rows` tail (:87) gains the window re-clamp — the function has no area
  access, so it only floors the OVER-SCROLL here (the caller's `keep_visible` and render's
  self-heal do the rest):
```rust
    if p.selected >= p.rows.len() { p.selected = p.rows.len().saturating_sub(1); }
    p.scroll_top = p.scroll_top.min(p.rows.len().saturating_sub(1));
```

- [ ] **Step 3: the palette key arms** (app.rs :1218-1292). A small local helper FIRST (place
  it beside `hydrate_overlays`, app.rs ~:818 — Task 2 reuses it for all four overlays):
```rust
/// Re-window an overlay list after a selection/rows change (A6). Reads the
/// active buffer's area height — the same source the mouse path uses; render
/// re-heals against the live frame each draw, so a transient divergence
/// (a key racing a resize) lasts at most one frame.
pub(crate) fn keep_overlay_visible(area_h: u16, selected: usize, row_count: usize, scroll_top: &mut usize) {
    let lh = crate::list_window::list_h_for(row_count, area_h);
    crate::list_window::keep_visible(selected, row_count, lh, scroll_top);
}
```
  Then in the palette block: the `Up` and `Down` arms, and the `Backspace`/`Char` arms (after
  their `rebuild_rows` calls), gain the re-window; and FOUR new arms slot beside them
  (before the `_ => {}` wildcard at :1287):
```rust
                    crossterm::event::KeyCode::Up => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            p.selected = p.selected.saturating_sub(1);
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::Down => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            let max = p.rows.len().saturating_sub(1);
                            p.selected = (p.selected + 1).min(max);
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::PageDown => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            let lh = crate::list_window::list_h_for(p.rows.len(), ah);
                            p.selected = (p.selected + lh.max(1)).min(p.rows.len().saturating_sub(1));
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::PageUp => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            let lh = crate::list_window::list_h_for(p.rows.len(), ah);
                            p.selected = p.selected.saturating_sub(lh.max(1));
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::Home => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            p.selected = 0;
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
                    crossterm::event::KeyCode::End => {
                        let ah = editor.active().view.area.1;
                        if let Some(p) = editor.palette.as_mut() {
                            p.selected = p.rows.len().saturating_sub(1);
                            keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                        }
                    }
```
  (Backspace/Char: after `crate::palette::rebuild_rows(p, reg, keymap);` add
  `keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);` — with the `ah`
  read hoisted above the `as_mut` borrow exactly as in the arms above. Esc/Enter/Left/Right
  UNCHANGED.)

- [ ] **Step 4: render — helpers + the windowed painter + the indicator.**
  - `palette_overlay_rect` (:150-159): replace the inline computation with
    `let list_h: u16 = crate::list_window::list_h_for(row_count, h) as u16;` (identical
    values — a pure dedup).
  - `palette_row_at` (:163-174): same `list_h_for` swap, and the return becomes
    `Some((row - list_top) as usize + palette.scroll_top)`.
  - The painter (:722-777): insert the SELF-HEAL pre-pass immediately before the
    `if let Some(ref palette)` binding:
```rust
    // A6 self-heal: the window must respect the LIVE frame's geometry (resize
    // has no overlay hook; render is the one place that always sees the truth).
    if let Some(p) = editor.palette.as_mut() {
        let lh = crate::list_window::list_h_for(p.rows.len(), h);
        crate::list_window::keep_visible(p.selected, p.rows.len(), lh, &mut p.scroll_top);
    }
```
    Inside the block: `let list_h = crate::list_window::list_h_for(palette.rows.len(), h) as u16;`
    (replacing the inline mirror at :730); the block construction gains the indicator:
```rust
        let mut block = Block::default().borders(Borders::ALL).title(" Command Palette ")
            .border_style(compose::compose(&editor.theme, editor.depth, &[SE::Chrome]));
        if palette.rows.len() > list_h as usize {
            block = block.title_bottom(
                ratatui::text::Line::from(format!(" {}/{} ", palette.selected + 1, palette.rows.len()))
                    .right_aligned(),
            );
        }
```
    and the items/select become windowed:
```rust
        let end = (palette.scroll_top + list_h as usize).min(palette.rows.len());
        let items: Vec<ListItem> = palette.rows[palette.scroll_top..end].iter().map(|row| {
            /* the existing label/chord/padding body VERBATIM */
        }).collect();

        let mut list_state = ListState::default();
        list_state.select(if palette.rows.is_empty() {
            None
        } else {
            Some(palette.selected.saturating_sub(palette.scroll_top))
        });
```
    (The `saturating_sub` is belt-and-braces — the pre-pass guarantees the invariant;
    NO debug_assert per the spec.)

- [ ] **Step 5: mouse — wheel + the click tests** (mouse.rs :122-145). Inside the palette
  block, BEFORE the `if let MouseEventKind::Down` (the block's unconditional return stays):
```rust
        match ev.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let ah = editor.active().view.area.1;
                if let Some(p) = editor.palette.as_mut() {
                    if matches!(ev.kind, MouseEventKind::ScrollDown) {
                        p.selected = (p.selected + 1).min(p.rows.len().saturating_sub(1));
                    } else {
                        p.selected = p.selected.saturating_sub(1);
                    }
                    crate::app::keep_overlay_visible(ah, p.selected, p.rows.len(), &mut p.scroll_top);
                }
                return;
            }
            _ => {}
        }
```
  The existing click test (`click_palette_row_dispatches_and_closes`, :446-465) gains an
  explicit `assert_eq!(ed.palette.as_ref().unwrap().scroll_top, 0)` precondition (same
  dispatch pinned — not weakened). NEW tests (mouse tests mod, the local `ctx()` idiom):
  - `scrolled_click_maps_to_absolute_row`: seed a Commands palette, set
    `selected = 20` + `keep_overlay_visible` (scroll_top 6), click the FIRST visible list
    row → the dispatched id is `rows[6]`'s (compute the cell from `palette_overlay_rect`).
  - `wheel_moves_selection_and_window`: 20 ScrollDowns → `selected == 20`,
    `selected - scroll_top < 15`, still open.

- [ ] **Step 6: unit + render + e2e tests.**
  - app tests: `palette_hazard_pin_enter_dispatches_visible_row` — open the palette
    (~110 rows, 80×24), drive `selected` to 50 via Down×50 (or End then Up×N), assert at
    dispatch time `p.selected == 50`, `p.selected - p.scroll_top < 15` (RED after the field
    lands / before keep_visible wiring — the honest state per the spec), then Enter and
    assert the dispatched command is `rows[50]`'s. Also `palette_pgdn_home_end_land_exactly`
    and `palette_filter_shrink_reclamps_window` (PgDn deep, type a narrowing char →
    `scroll_top` re-clamped, selection visible). Also the Buffers case (g):
    seed 20 buffers via the switcher path, PgDn → highlight visible.
  - render tests: `palette_windowed_slice_shows_scrolled_rows` (set selected=50 +
    heal via a draw; assert a row string matches `rows[scroll_top]`'s label, NOT rows[0]'s);
    `palette_indicator_only_when_scrollable` (31 rows → bottom border contains "51/…"?? —
    use selected=12 → ` 13/31 `; 3 rows → NO indicator text in the bottom border);
    `palette_resize_self_heal_no_panic` (selected=50, scroll_top=36 seeded manually, draw
    into an 80×10 backend → no panic, and after the draw `selected - scroll_top < list_h_for(rows, 10)`);
    the h=4 degenerate draw (no panic, no rows painted).
  - e2e journey (`journey_palette_end_reaches_last_command`): `ctrl('p')` → `key(End)` →
    assert the highlight visible (`selected - scroll_top < 15`) and the LAST row's label on
    screen → `key(Enter)` → the dispatched command ran (pick a benign last-registered
    command to assert on — verify what the registry's last command is and assert its
    observable effect or use `screen_contains` on the closed palette + status; the
    implementer picks the cleanest observable, the reach-without-typing property is the
    contract).

- [ ] **Step 7: run + gates + commit.**
```bash
git add -A
git commit -m "feat(palette): windowed scrolling — full-list reach, wheel, position indicator (A6 T1)"   # + trailers
```

---

### Task 2: the three siblings (outline, theme picker, file browser)

**Files:**
- Modify: `wordcartel/src/outline_overlay.rs`, `theme_picker.rs`, `file_browser.rs` (fields +
  literal sites), `wordcartel/src/app.rs` (three key blocks + the descend reset),
  `wordcartel/src/render.rs` (three painters), `wordcartel/src/mouse.rs` (tp/fb wheel),
  `wordcartel/src/editor.rs` (the two open-site literals).

**Interfaces:** consumes Task 1's `list_window` + `keep_overlay_visible` + the painter pattern.

- [ ] **Step 1: fields + literals.** Add the same doc-commented `pub scroll_top: usize` to
  `OutlineOverlay` (outline_overlay.rs:14-22), `ThemePicker` (theme_picker.rs:7-13),
  `FileBrowser` (file_browser.rs:14-19). Struct-literal sites gain `scroll_top: 0`:
  editor.rs:675-678 (theme picker open), :725-727 (file browser open),
  theme_picker.rs:33-34 (test), file_browser.rs:64 (test), and any outline construction
  site (find via the compiler — OutlineOverlay's ctor path). The `rebuild_rows`
  (theme_picker.rs:17-24), `rebuild_entries` (file_browser.rs:23), and `set_query`
  (outline_overlay.rs:45-50) tails each gain the same over-scroll floor as palette's:
  `x.scroll_top = x.scroll_top.min(x.rows.len().saturating_sub(1));` (entries for fb).

- [ ] **Step 2: key arms — same shape as Task 1 Step 3, per block:**
  - Theme picker (app.rs:1316-1360): Up/Down gain `keep_overlay_visible` INSIDE the
    `as_mut` scope, **BEFORE the `preview_selected_theme(editor)` call that follows the
    scope** (the ordering pin — the previewed row must be the visible one); Backspace/Char
    likewise re-window after `rebuild_rows` and before preview; new
    PageDown/PageUp/Home/End arms (each ending with `preview_selected_theme(editor);`).
  - File browser (app.rs:1379-1450): Up/Down + re-window; new PgUp/PgDn/Home/End;
    Backspace/Char re-window after `rebuild_entries`; **THE DESCEND RESET (spec C1)** — in
    the Enter arm's directory branch:
```rust
                                        if let Some(fb) = editor.file_browser.as_mut() {
                                            fb.dir = target;
                                            fb.query.clear();
                                            fb.selected = 0;
                                            fb.scroll_top = 0; // A6: a stale window over a
                                            // smaller entry set would make the render slice
                                            // out-of-order (panic-class) — reset with selected.
                                            crate::file_browser::rebuild_entries(fb);
                                        }
```
  - Outline (app.rs:1642-1700): Up/Down + re-window; new PgUp/PgDn/Home/End; the
    Backspace/Char arms re-window after `set_query` (the `ah` read hoisted before the
    `as_mut`, as everywhere). Esc/Enter untouched.

- [ ] **Step 3: render — replicate Task 1's painter pattern at the three sites** (outline
  :781-822, theme picker :827-868, file browser :872-914): the self-heal pre-pass
  (an `as_mut` block before each `if let Some(ref …)`), `list_h` via `list_h_for`, the
  windowed slice + relative select (each keeps its existing per-row item body VERBATIM),
  and the conditional `title_bottom` indicator (file browser's block already has a dynamic
  title — `title_bottom` composes with it).

- [ ] **Step 4: mouse — tp/fb wheel** (mouse.rs:174-179): each bare-return block becomes:
```rust
    if editor.theme_picker.is_some() {
        match ev.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let ah = editor.active().view.area.1;
                if let Some(tp) = editor.theme_picker.as_mut() {
                    if matches!(ev.kind, MouseEventKind::ScrollDown) {
                        tp.selected = (tp.selected + 1).min(tp.rows.len().saturating_sub(1));
                    } else {
                        tp.selected = tp.selected.saturating_sub(1);
                    }
                    crate::app::keep_overlay_visible(ah, tp.selected, tp.rows.len(), &mut tp.scroll_top);
                }
                crate::app::preview_selected_theme_pub(editor); // see note below
                return;
            }
            _ => {}
        }
        return;
    }
```
  (`preview_selected_theme` is private to app.rs (:1086) — expose a `pub(crate)` wrapper or
  make it `pub(crate)`; the wheel-preview must follow the same visible-row ordering. The
  file-browser block: identical minus the preview line. The unconditional `return` stays in
  BOTH blocks — they still protect the text area from all other mouse interaction.)

- [ ] **Step 5: tests.**
  - Sibling (a)-(c) equivalents (app tests): outline with 25 headings + fb with 25 entries
    (tempdir) — Down past the window keeps `selected - scroll_top < list_h`; PgDn/Home/End
    land exactly; **the fb Enter-dispatches-visible pin** (selected deep → the opened path
    is `entries[selected]`'s, visible at dispatch).
  - **The scrolled-descend pin (spec C1, panic-class):** fb over a 25-entry tempdir, PgDn
    (scroll_top > 0), Enter on a subdirectory containing 2 entries → no panic, `selected == 0`,
    `scroll_top == 0`, render draws clean.
  - **The theme-picker preview pin (spec I4 construction):** pad `tp.rows` with REPEATED
    REAL builtin names to 30 rows (e.g. cycling `Theme::builtin_names()`), drive with
    NAVIGATION ONLY (Down×20 — no Char/Backspace, which would rebuild and wipe the padding),
    assert the applied theme's name equals `tp.rows[tp.selected]` AND
    `tp.selected - tp.scroll_top < list_h` (RED before the ordering/keep_visible wiring).
  - tp/fb wheel tests (mouse mod): wheel moves selection + window; tp wheel also previews
    the visible row.
  - One sibling render check: outline windowed slice shows scrolled rows (mirror of T1's).

- [ ] **Step 6: run + gates + commit.**
```bash
git commit -m "feat(overlays): windowed scrolling for outline/theme-picker/file-browser (A6 T2)"   # + trailers
```

---

## Pre-merge checklist (beyond the standard gates)

1. `scripts/smoke/run.sh` — one-line summary quoted verbatim (advisory).
2. Live tmux sanity: open wcartel, `ctrl-p`, press `End` — the last command is visible and
   highlighted, the indicator reads `{n}/{n}`; PgUp walks back with the highlight always
   visible.
3. Ship-time bookkeeping (from the spec): add the "overlay mouse parity" follow-up to
   `docs/ux-backlog.md` (tp/fb click-to-select; an outline mouse block); mark A6 SHIPPED and
   A3 reduced to the curation pass.

## Self-Review

**Spec coverage:** the shared module with exact semantics + the three-table unit tests ✓;
scroll_top on all four + every literal site enumerated (spec M5) ✓; saturating forms
everywhere (spec I3) ✓; palette keys incl. the hoisted-`ah` borrow shape + rebuild
re-windows ✓; the descend reset with the panic rationale (spec C1) ✓; render self-heal
pre-pass per painter, NO debug_assert, windowed slice + relative select with the item bodies
kept verbatim (user decision A) ✓; the indicator via `title_bottom(...right_aligned())`
only-when-scrollable, composing with fb's dynamic title ✓; `palette_row_at` + scroll_top and
the click-test adjustment-not-weakening ✓; wheel-moves-selection in the three existing
blocks with returns preserved + the tp wheel-preview ordering ✓; the preview pin's I4
construction constraints (repeated real names, navigation-only) ✓; the hazard pins with
honest red states (spec M6) ✓; the resize/degenerate pins (spec h) ✓; the Buffers case (g)
✓; the e2e End journey ✓; smoke + tmux sanity + ship-time bookkeeping ✓.

**Placeholder scan:** two implementer-choice spots are contract-bounded: the e2e journey's
"benign last command" observable (the reach property is the contract) and
`preview_selected_theme`'s visibility change (pub(crate) vs a wrapper — either form, same
call). Everything else is complete code.

**Type consistency:** `list_h_for(usize, u16) -> usize`;
`keep_visible(usize, usize, usize, &mut usize)`;
`keep_overlay_visible(u16, usize, usize, &mut usize)` (app.rs, pub(crate), reused by mouse
via `crate::app::`); render casts `list_h_for(...) as u16` where rect math needs u16;
`Line::right_aligned()` per ratatui 0.30 (verified in the vendored source at review time).

**Ordering:** T1 establishes module + pattern + the app.rs helper; T2 replicates. Sequential;
both touch app.rs/render.rs/mouse.rs in disjoint blocks.
