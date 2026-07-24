# Group ③ — Degenerate-Geometry Safety Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship B9 (responsive menu-bar label-compression ladder + `»` overflow marker + dropdown shift-left-to-fit) and the H23 rider (the `palette_overlay_rect` u32 widen + census comment), per the locked spec `docs/superpowers/specs/2026-07-24-group3-degenerate-geometry-design.md` (Codex-clean, round 1).

**Architecture:** All bar geometry stays PURE and single-sourced in `chrome_geom.rs`: a three-rung ladder (`BarRung`) selected per frame from `(area.width, cats)`, one cell-text producer that both layout and paint consume, one consolidated bar hit-test (`menu_bar_label_at`) that replaces three hand-parallel mouse closures, and a marker column derived from the same layout — zero stored state, so paint and mouse cannot drift (A21/H21 lesson made structural). H23 is a one-line widen whose safety argument is exactness, not assertion.

**Tech Stack:** Rust (`wordcartel` shell crate only — no core change), ratatui 0.30 `TestBackend` render tests, existing mouse-test harness (`ctx()`/`handle_flat`/`down`/`moved`).

**Branch:** `effort-group3-geometry` (created in Task 1, off `main` @ `1a994ad`).

## Global Constraints

- **Anchor by SYMBOL NAME, never line number** — locate every edit site via the named symbol (grep / `workspaceSymbol`); recorded `:NNN` anchors drift.
- **Do NOT run `cargo fmt`** — the repo is hand-formatted with no `rustfmt.toml`; match neighboring style by hand (4-space indent, ~100-char hand-wrapped lines with judgment, `—` em-dash in prose comments, imports grouped by hand, no emoji in code).
- **GATEs every task must pass before its commit:** `cargo test --workspace` green; `cargo build` and `cargo test --no-run` warning-free for `wordcartel`; `cargo clippy --workspace --all-targets` clean (workspace denies `clippy::all`; `too_many_lines` threshold 100); module budgets run inside `cargo test` (`wordcartel/tests/module_budgets.rs` — note `render.rs` budget 900 is tight: production is 897 lines today (`mod tests` opens at the 898th), and this plan adds exactly one production comment (TWO physical lines, Task 1.4) → 899/900; all other `render.rs` additions are `#[cfg(test)]`).
- **H23 guard form (human-approved deviation from the filing's letter):** plain u32 widen, **NO `debug_assert`**. The narrowing is exact for every u16 input ((65535·3)/5 = 39321 < 65536), so there is no invariant left to assert — do not "helpfully" re-add an assert; spec §4.4 records the human's approval (2026-07-24).
- **Locked B9 strings and thresholds (do not re-derive, do not re-negotiate):** short labels `Blk`/`Fmt`/`Docs`/`Set`/`Exp` (File/Edit/View stay full); rungs for today's 8 categories: FullPadded ≥ 62 cols, Full 54–61, Short 36–53, clipped-with-`»` 4–35 (`render`'s §15.6 guard owns < 4). Thresholds are COMPUTED from the label strings at runtime; the tests pin today's derived values.
- **Perf law:** per-keystroke work stays O(visible)+O(edited). The ladder is a pure per-frame function over ≤ 8 static labels (a few dozen char-counts) on the render path only; no allocation beyond the existing per-frame label `format!`s; no stored state, no timers, no background work.
- **Command-surface contract: N/A** (spec §5) — geometry/paint/hit-test only. No command added/removed/renamed/rebound; no user-settable option (compression is automatic-responsive BY DESIGN so no option exists to need a command); palette/menu membership, dynamic sections, and hints untouched; the contract's invariant tests remain green unmodified.
- **Editor hints lie about fresh edits:** for compile/usage questions on code you are editing, trust `cargo` + `grep`, never an editor "unused"/"undefined" diagnostic.
- **Do not modify existing tests** except where a task explicitly says so; spec §6f predicts ZERO churn in existing menu/dropdown tests — a needed edit to one is a spec-compliance red flag to STOP and report, not fix.
- **Every commit ends with BOTH project trailers, verbatim, in this order:**
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01FAx3iA5vRBiLXEfCnudR6j
  ```
- **Ledger:** after each task, append one line (task id + commit hash) to `$(git rev-parse --git-path sdd)/progress.md`.

## File Map

| File | Change |
|---|---|
| `wordcartel/src/chrome_geom.rs` | H23 widen (Task 1); `BarRung` + `menu_bar_cell_text` + `menu_bar_rung` + rung-aware `menu_bar_layout_cats` (Task 2); `menu_bar_marker_col` + `menu_bar_label_at` + `menu_bar_hit` (Task 3); `menu_dropdown_rect` shift-left (Task 4); tests in every task |
| `wordcartel/src/menu.rs` | `category_label_short` (Task 2) |
| `wordcartel/src/render_overlays.rs` | `paint_menu_bar` both arms use cell text (Task 2); marker paint + `Modifier` import (Task 3) |
| `wordcartel/src/mouse.rs` | 3 bar-hit sites migrate to the shared helper (Task 3); narrow-bar mouse tests (Task 3) |
| `wordcartel/src/render.rs` | 1-line census comment at the wrap-guide site (Task 1); narrow-bar ladder render tests (Tasks 2–3); extreme-width render test (Task 5) — tests are `#[cfg(test)]`, budget-neutral |

## Task Order & Rationale

1. **Task 1 — branch + H23 seed.** Smallest, fully independent; red test documents the debug-panic boundary before the fix exists; every later task's full-render tests then run on a seed-safe helper.
2. **Task 2 — B9 ladder core (layout + paint).** The feature's heart. Mouse sites still consume the SAME rects via `menu_bar_layout*`, so hit-tests stay consistent mid-plan without touching mouse.rs — the intermediate state is coherent and green.
3. **Task 3 — marker + consolidated hit-test.** Needs Task 2's rungs (the marker means "even the floor rung clips"). Migrates the three closures and lands the clipped-width behavior.
4. **Task 4 — dropdown shift-left.** Independent of Tasks 2–3 logically, but landing after them lets its test exercise the real compressed-bar label positions.
5. **Task 5 — extreme-width belt-and-braces.** Last so the 21846×4 full render sweeps ALL the new paint code.
6. **Task 6 — final verification** (no merge; whole-branch gates follow per process).

---

### Task 1: Branch + H23 seed widen + census comment

- [ ] **1.1** Create the branch off main:
  ```sh
  git checkout main && git pull --ff-only && git checkout -b effort-group3-geometry
  ```
  Confirm `git log -1 --format=%h` shows `1a994ad` (or a fast-forwarded descendant — STOP and report if main moved past the spec's grounding).

- [ ] **1.2 RED:** in `wordcartel/src/chrome_geom.rs`'s `#[cfg(test)] mod tests`, add (near `palette_overlay_rect_sizes_to_row_count`):
  ```rust
  /// H23 (spec §6a): the overlay width math survives every u16 width. The seed defect was
  /// `w * 3` overflowing u16 at w >= 21846 (debug panic; release wrap-then-clamp) — 21845 is
  /// the last width whose w*3 fits u16, so the pair documents the boundary.
  #[test]
  fn palette_overlay_rect_survives_extreme_widths() {
      for w in [21845u16, 21846, 30000, u16::MAX] {
          let area = Rect::new(0, 0, w, 24);
          let r = palette_overlay_rect(area, 10);
          assert!(r.width >= 30 && r.width <= 80.min(w),
              "w={w}: ov_w {} outside [30, min(80, w)]", r.width);
          assert!(r.x as u32 + r.width as u32 <= w as u32, "w={w}: past the right edge: {r:?}");
          assert!(r.y as u32 + r.height as u32 <= 24, "w={w}: past the bottom edge: {r:?}");
      }
  }
  ```
  Run `cargo test -p wordcartel palette_overlay_rect_survives` — MUST FAIL (debug overflow panic
  `attempt to multiply with overflow` at the `w * 3` for 21846). Quote the failure.

- [ ] **1.3 GREEN:** in `chrome_geom::palette_overlay_rect`, replace the width line:
  ```rust
  let ov_w = (w * 3 / 5).clamp(30, 80).min(w);
  ```
  with:
  ```rust
  // H23: widen the *3/5 to u32 — `w * 3` overflows u16 at w >= 21846 (hostile Resize).
  // The narrowing back is exact ((65535*3)/5 = 39321 < 65536); no debug_assert on purpose —
  // nothing is left to assert after widening (spec §4.4, human-approved).
  let ov_w = ((w as u32 * 3 / 5) as u16).clamp(30, 80).min(w);
  ```
  Re-run the test — green.

- [ ] **1.4** In `wordcartel/src/render.rs`, locate the wrap-guide block (symbol anchor: the
  `if editor.view_opts.wrap_guide {` block inside `render`), and add ONE comment line directly
  above `let gx = area.x + tg.text_left + editor.view_opts.wrap_column;`:
  ```rust
  // H23 census: raw adds safe — wrap_column is clamped to [20, 9999] at both entry
  // points (config.rs load; prompts::wrap_column_submit) and text_left < w/2.
  ```
  (One comment, hand-wrapped to TWO physical lines per the ~100-char house style — the
  resolved decision's "one-line comment" in substance. `render.rs` production goes 897 → 899,
  under the 900 hub budget; this is the plan's only `render.rs` production change. If the
  budget test fires anyway, STOP and report — do not trim other production code to
  compensate.)

- [ ] **1.5** Gates: `cargo test --workspace` green, `cargo build` + `cargo test --no-run`
  warning-free, `cargo clippy --workspace --all-targets` clean.

- [ ] **1.6** Commit:
  ```
  group3 T1: H23 — widen palette_overlay_rect's *3/5 to u32; census comment at the wrap guide

  The only u16 geometry multiply in the crate (spec §4.2 census). Plain widen, no
  debug_assert — the narrowing is exact for every u16 width (spec §4.4).
  ```
  (+ the two trailers). Ledger line.

### Task 2: B9 ladder core — rungs, short labels, rung-aware layout, paint

- [ ] **2.1** In `wordcartel/src/menu.rs`, directly below `category_label_pub`, add:
  ```rust
  /// Short bar labels for the compressed rung (B9, spec §3.2 — LOCKED strings): only long
  /// categories shorten; File/Edit/View stay full. Exhaustive on purpose — a new category
  /// must consciously pick its short form (house pattern-matching rule).
  pub(crate) fn category_label_short(cat: MenuCategory) -> &'static str {
      match cat {
          MenuCategory::File => "File",
          MenuCategory::Edit => "Edit",
          MenuCategory::Block => "Blk",
          MenuCategory::Format => "Fmt",
          MenuCategory::View => "View",
          MenuCategory::Documents => "Docs",
          MenuCategory::Settings => "Set",
          MenuCategory::Export => "Exp",
      }
  }
  ```
  (No `_pub` wrapper pair — that suffix exists on `category_label_pub` only because the full-label
  fn predates it as private; a single `pub(crate)` fn is the clean form for new code.)

- [ ] **2.2 RED (unit):** in `chrome_geom.rs` tests:
  ```rust
  /// B9 (spec §3.2): the three-rung ladder's derived thresholds for today's 8 categories.
  /// A future label/category change must consciously re-derive this table (and spec §3.2).
  #[test]
  fn menu_bar_rung_thresholds_for_the_eight_categories() {
      use crate::registry::MENU_ORDER;
      let at = |w: u16| menu_bar_rung(Rect::new(0, 0, w, 8), &MENU_ORDER);
      assert_eq!(at(80), BarRung::FullPadded);
      assert_eq!(at(62), BarRung::FullPadded, "62 is the full-padded total");
      assert_eq!(at(61), BarRung::Full);
      assert_eq!(at(54), BarRung::Full, "54 is the full+leading-space total");
      assert_eq!(at(53), BarRung::Short);
      assert_eq!(at(36), BarRung::Short, "36 is the short total");
      assert_eq!(at(35), BarRung::Short, "below the floor the bar stays Short and clips");
  }

  /// B9: at the Short floor every category fits exactly — the bar ends flush at col 36.
  #[test]
  fn menu_bar_layout_fits_every_category_at_the_short_floor() {
      use crate::registry::MENU_ORDER;
      let bar = menu_bar_layout_cats(Rect::new(0, 0, 36, 8), &MENU_ORDER);
      assert_eq!(bar.len(), 8);
      let (_, last) = bar.last().expect("eight rects");
      assert_eq!(last.x + last.width, 36, "compressed bar ends flush at 36 cols");
  }
  ```
  MUST FAIL to compile (`menu_bar_rung`/`BarRung` don't exist) — the red state.

- [ ] **2.3 RED (render):** in `render.rs` tests (near
  `the_prompt_detail_box_never_panics_or_overflows_a_tiny_terminal`, reusing `render_to_buffer` +
  `row_string`):
  ```rust
  /// B9 (spec §6b): the bar compresses through the §3.2 ladder instead of clipping. Widths
  /// cover both sides of every rung boundary; the clipped widths {10, 20, 35} join in the
  /// marker test once `»` exists (Task 3).
  #[test]
  fn the_menu_bar_compresses_through_the_ladder_instead_of_clipping() {
      use crate::config::MenuBarMode;
      for (w, expect, absent) in [
          (80u16, " Documents  Settings ", ""),
          (62, " Documents  Settings ", ""),
          (61, "Documents Settings Export", " Documents  "),
          (54, "Documents Settings Export", " Documents  "),
          (53, "Blk Fmt View Docs Set Exp", "Documents"),
          (36, " File Edit Blk Fmt View Docs Set Exp", "Documents"),
      ] {
          let mut e = Editor::new_from_text("hello\n", None, (w, 8));
          e.menu_bar_mode = MenuBarMode::Pinned;
          e.menu = None;
          derive::rebuild(&mut e);
          let buf = render_to_buffer(&mut e, w, 8);
          assert_eq!(buf.area.width, w, "{w}: painter must not resize the frame");
          let row0 = row_string(&buf, 0);
          assert!(row0.contains(expect), "{w} cols: bar {row0:?} must contain {expect:?}");
          if !absent.is_empty() {
              assert!(!row0.contains(absent), "{w} cols: bar {row0:?} must NOT contain {absent:?}");
          }
      }
  }
  ```
  Red choreography for this task: 2.2 + 2.3 are written first (2.2 fails to COMPILE — the new
  symbols don't exist; that is the red run, quote it). After 2.4 lands, run 2.3 alone: the
  61/54 cases go green (a FullPadded cell text clipped to a Full-rung cell coincidentally equals
  the Full text) but 53/36 stay RED — the painter still formats full labels into Short cells
  (" Blo"/" For" truncations, no "Blk Fmt") — quote that too; 2.5 turns the whole test green.
  This staged red proves the test discriminates layout from paint.

- [ ] **2.4 Impl (geometry):** in `chrome_geom.rs`, directly above `menu_bar_layout_cats`:
  ```rust
  /// B9 — the responsive bar's ladder rung, widest-first (spec §3.2). Selected per frame as
  /// a pure function of (area.width, cats): no stored state, so paint and mouse re-derive
  /// identical geometry every frame (the A21/H21 lockstep lesson made structural).
  #[derive(Clone, Copy, PartialEq, Eq, Debug)]
  pub(crate) enum BarRung { FullPadded, Full, Short }

  /// The ONE producer of a bar cell's text (spec §3.3). Layout MEASURES this string and the
  /// painter RENDERS it, so cell width and painted width agree by construction.
  pub(crate) fn menu_bar_cell_text(cat: crate::registry::MenuCategory, rung: BarRung) -> String {
      match rung {
          BarRung::FullPadded => format!(" {} ", crate::menu::category_label_pub(cat)),
          BarRung::Full => format!(" {}", crate::menu::category_label_pub(cat)),
          BarRung::Short => format!(" {}", crate::menu::category_label_short(cat)),
      }
  }

  /// Widest rung whose summed cell widths fit `area.width`; `Short` is the floor (below its
  /// total the bar clips and the `»` marker appears). Thresholds are COMPUTED from the label
  /// strings — for today's eight categories: FullPadded ≥ 62, Full ≥ 54, Short ≥ 36.
  pub(crate) fn menu_bar_rung(area: Rect, cats: &[crate::registry::MenuCategory]) -> BarRung {
      let total = |rung: BarRung| -> u32 {
          cats.iter().map(|c| menu_bar_cell_text(*c, rung).chars().count() as u32).sum()
      };
      if total(BarRung::FullPadded) <= area.width as u32 { BarRung::FullPadded }
      else if total(BarRung::Full) <= area.width as u32 { BarRung::Full }
      else { BarRung::Short }
  }
  ```
  Then make `menu_bar_layout_cats` rung-aware (same signature — every caller compiles unchanged):
  ```rust
  pub(crate) fn menu_bar_layout_cats(area: Rect, cats: &[crate::registry::MenuCategory]) -> Vec<(usize, Rect)> {
      let rung = menu_bar_rung(area, cats);
      let mut out = Vec::new();
      let mut x = area.x;
      for (i, cat) in cats.iter().enumerate() {
          let wgt = menu_bar_cell_text(*cat, rung).chars().count() as u16;
          out.push((i, Rect::new(x, area.y, wgt, 1)));
          x = x.saturating_add(wgt);
      }
      out
  }
  ```
  (The doc comment on the fn keeps its first two lines; drop the now-stale `+ 2 // 1 space padding`
  wording.)

- [ ] **2.5 Impl (paint):** in `render_overlays.rs::paint_menu_bar`, add `menu_bar_cell_text,
  menu_bar_rung,` to the existing `crate::chrome_geom::{…}` import list, then rewrite the two
  arms' label loops:
  ```rust
      match editor.menu {
          Some(ref menu) if !menu.groups.is_empty() => {
              // Paint the menu bar (one label per category) at the responsive rung (B9).
              let cats: Vec<crate::registry::MenuCategory> =
                  menu.groups.iter().map(|g| g.0).collect();
              let rung = menu_bar_rung(menu_area, &cats);
              let bar = menu_bar_layout(menu_area, &menu.groups);
              for (i, rect) in &bar {
                  let text = menu_bar_cell_text(menu.groups[*i].0, rung);
                  let style = if *i == menu.open { cs.menu_open } else { cs.menu_closed };
                  frame.render_widget(Paragraph::new(text).style(style), *rect);
              }
          }
          _ => {
              // Inactive bar (pinned / auto-revealed / unbuilt placeholder): static
              // labels, all closed-style, no dropdown, no highlight — same rung ladder.
              let rung = menu_bar_rung(menu_area, &crate::registry::MENU_ORDER);
              for (i, rect) in &menu_bar_layout_cats(menu_area, &crate::registry::MENU_ORDER) {
                  let text = menu_bar_cell_text(crate::registry::MENU_ORDER[*i], rung);
                  frame.render_widget(Paragraph::new(text).style(cs.menu_closed), *rect);
              }
          }
      }
  ```

- [ ] **2.6** All Task-2 tests green; FULL suite green (spec §6f: existing menu/dropdown/mouse
  tests must pass UNMODIFIED — they all run ≥ 62-col or single-category bars where FullPadded
  reproduces today's layout byte-identically; any red among them = STOP, report).

- [ ] **2.7** Gates (build/clippy/no-run). Commit:
  ```
  group3 T2: B9 — three-rung responsive bar ladder (full/tight/abbreviated), pure per-frame

  Rung + cell text single-sourced in chrome_geom; menu_bar_layout_cats keeps its
  signature so all five consumers stay in lockstep. Locked short labels (spec §3.2):
  Blk/Fmt/Docs/Set/Exp.
  ```
  (+ trailers). Ledger line.

### Task 3: `»` overflow marker + consolidated bar hit-test (3 mouse sites)

**TDD honesty note (Codex plan-gate round 1):** the 3-closure → `menu_bar_label_at`
consolidation is a **behavior-preserving refactor** for the existing arms — after Task 2 the old
closures already consume the rung-aware rects, so compressed-cell clicks and hovers work TODAY
through them; like H27's `handle_flat` migration, the existing green suite plus green-on-arrival
PINS (step 3.6) are the oracle, and no red-first exists for that part. The genuinely NEW
behavior — and therefore the red-first tests (step 3.1) — is the `»` marker. **Red baseline =
the POST-T2, pre-migration tree** (this task runs after Task 2, so the Short-rung layout is
already in place when 3.1 first runs): no marker glyph paints, and a click on the marker column
OPENS the partially-clipped category whose Short cell covers it — at 35 cols `Exp`'s cell spans
[32, 36) so col 34 opens Export through the un-migrated `CellHit::MenuBar` closure. (Against
the pre-plan full-label bar the same columns land differently — col 34 in `Documents`'s
[33, 44) — which is why the baseline task-state must be named: the ranges below are POST-T2
facts, not current-main facts.) After this task the marker column must be inert.

- [ ] **3.1 RED (behavioral — red AS SEQUENCED: introduced and run on the post-T2 tree,
  before this task's migration/paint; both tests reference no new symbols, so they compile
  there and fail for the stated reasons):**

  In `render.rs` tests (sibling of Task 2.3's test):
  ```rust
  /// B9 (spec §6b, clipped widths): below the Short floor the bar clips and the dim `»`
  /// marker owns the last bar column; at and above the floor there is no marker.
  /// RED as sequenced (post-T2, pre-marker-paint): no `»` paints yet — the last-column
  /// cell is a clipped Short-label glyph.
  #[test]
  fn below_the_floor_the_bar_clips_and_shows_the_overflow_marker() {
      use crate::config::MenuBarMode;
      use ratatui::style::Modifier;
      for w in [35u16, 20, 10] {
          let mut e = Editor::new_from_text("hello\n", None, (w, 8));
          e.menu_bar_mode = MenuBarMode::Pinned;
          e.menu = None;
          derive::rebuild(&mut e);
          let buf = render_to_buffer(&mut e, w, 8);
          let row0 = row_string(&buf, 0);
          assert!(row0.starts_with(" File"), "{w}: bar still leads with File: {row0:?}");
          assert_eq!(row0.chars().last(), Some('»'), "{w}: marker in the last column: {row0:?}");
          assert!(buf[(w - 1, 0)].style().add_modifier.contains(Modifier::DIM),
              "{w}: the marker cell is DIM");
      }
      for w in [36u16, 53, 80] {
          let mut e = Editor::new_from_text("hello\n", None, (w, 8));
          e.menu_bar_mode = MenuBarMode::Pinned;
          e.menu = None;
          derive::rebuild(&mut e);
          let row0 = row_string(&render_to_buffer(&mut e, w, 8), 0);
          assert!(!row0.contains('»'), "{w}: no marker at or above the floor: {row0:?}");
      }
  }
  ```

  In `mouse.rs` tests (reusing `ctx()`, `handle_flat`, `down`). The marker column is derived
  as `w - 1` directly (the spec: the marker owns the LAST bar column when clipped) so this
  test compiles and runs RED on the post-T2 tree without referencing any Task-3 symbol —
  `menu_bar_marker_col` equality is pinned separately in 3.2's unit test. Its red-ness
  DEPENDS on T2's rung-aware layout being present (the asserted cell ranges are Short-rung
  ranges); do not cherry-pick it onto an earlier tree:
  ```rust
  /// B9 (spec §6c, I5): on a clipped bar the last column belongs to the `»` marker and a
  /// click there is INERT. RED as sequenced (post-T2 layout, pre-T3 migration): at 35 cols
  /// Exp's SHORT-rung cell spans [32, 36), so the un-migrated CellHit::MenuBar closure opens
  /// Export from col 34; after the migration the marker column returns no label. Cols 32–33
  /// (Exp's still-visible cells) keep opening it, before and after.
  #[test]
  fn click_on_the_marker_column_is_inert_but_visible_cells_still_open() {
      use crate::config::MenuBarMode;
      let (reg, ex, clk, tx, km) = ctx();
      let mk = |w: u16| {
          let mut e = Editor::new_from_text("hello\n", None, (w, 8));
          crate::derive::rebuild(&mut e);
          e.menu_bar_mode = MenuBarMode::Pinned;
          e.menu = None;
          e
      };
      // The marker column (last bar column on a clipped bar) is inert.
      let mut e = mk(35);
      handle_flat(&mut e, down(34, 0), &reg, &km, &ex, &clk, &tx, &crate::test_support::test_fs());
      assert!(e.menu.is_none(), "a click on the » marker column must do nothing");
      // A still-visible cell of the same partially-clipped category still opens it.
      let mut e = mk(35);
      handle_flat(&mut e, down(32, 0), &reg, &km, &ex, &clk, &tx, &crate::test_support::test_fs());
      assert_eq!(e.menu.as_ref().map(|m| m.open), Some(7),
          "Exp's visible cells (32–33) still open Export");
      // Narrower: at 20 cols the last column (19) sits inside View's cell [18, 23) — inert too.
      let mut e = mk(20);
      handle_flat(&mut e, down(19, 0), &reg, &km, &ex, &clk, &tx, &crate::test_support::test_fs());
      assert!(e.menu.is_none(), "the marker column is inert at every clipped width");
  }
  ```
  Run both ON THE POST-T2 TREE — MUST FAIL (the render test on the missing `»`; the mouse
  test with `e.menu = Some(open: 7)` at col 34 — Exp's Short cell — and `Some(open: 4)` at
  col 19 — View's Short cell [18, 23)). Quote both failures — they prove the marker behavior
  does not exist yet at the point in the sequence where it is about to be built.

- [ ] **3.2 RED (compile — new helper unit tests):** `chrome_geom.rs` tests:
  ```rust
  /// B9 (spec §3.4): the marker exists exactly when even the Short floor clips.
  #[test]
  fn menu_bar_marker_col_appears_only_below_the_short_floor() {
      use crate::registry::MENU_ORDER;
      assert_eq!(menu_bar_marker_col(Rect::new(0, 0, 80, 8), &MENU_ORDER), None);
      assert_eq!(menu_bar_marker_col(Rect::new(0, 0, 36, 8), &MENU_ORDER), None);
      assert_eq!(menu_bar_marker_col(Rect::new(0, 0, 35, 8), &MENU_ORDER), Some(34));
      assert_eq!(menu_bar_marker_col(Rect::new(0, 0, 10, 8), &MENU_ORDER), Some(9));
  }

  /// B9 (I5): every category is hittable at its cell; the marker column never is, even
  /// though a clipped label rect covers it.
  #[test]
  fn menu_bar_label_at_hits_every_category_and_never_the_marker() {
      use crate::registry::MENU_ORDER;
      let area = Rect::new(0, 0, 40, 8); // Short rung — everything visible
      for (i, r) in menu_bar_layout_cats(area, &MENU_ORDER) {
          assert_eq!(menu_bar_label_at(area, &MENU_ORDER, r.x + 1, 0), Some(i),
              "category {i} must be hittable at its cell");
      }
      assert_eq!(menu_bar_label_at(area, &MENU_ORDER, 39, 0), None, "off the flush bar end");
      let clipped = Rect::new(0, 0, 35, 8);
      let mc = menu_bar_marker_col(clipped, &MENU_ORDER).expect("clipped at 35");
      assert_eq!(menu_bar_label_at(clipped, &MENU_ORDER, mc, 0), None,
          "the marker column is not a label hit");
  }
  ```
  Fails to compile (helpers don't exist) — red.

- [ ] **3.3 Impl (geometry):** `chrome_geom.rs`, below `menu_bar_layout`:
  ```rust
  /// B9 (spec §3.4) — Some(column of the bar's LAST cell) iff even the floor rung clips
  /// (the last label rect's right edge exceeds the area's); None when everything fits. The
  /// painter draws the dim `»` there and `menu_bar_label_at` treats the same column as
  /// label-free, so the marker and its dead column can never disagree (I5).
  pub(crate) fn menu_bar_marker_col(area: Rect, cats: &[crate::registry::MenuCategory]) -> Option<u16> {
      if area.width == 0 { return None; }
      let clipped = menu_bar_layout_cats(area, cats).last()
          .is_some_and(|(_, r)| r.right() > area.right());
      clipped.then(|| area.right().saturating_sub(1))
  }

  /// B9 — THE bar hit-test (replaces the three hand-parallel find-closures in mouse.rs:
  /// mouse_menu's Moved and Down arms + the inactive CellHit::MenuBar arm). Returns the
  /// index into `cats` whose label cell contains `(col, row)`; None off every label or on
  /// the active marker column.
  pub(crate) fn menu_bar_label_at(area: Rect, cats: &[crate::registry::MenuCategory],
      col: u16, row: u16) -> Option<usize>
  {
      if menu_bar_marker_col(area, cats) == Some(col) { return None; }
      menu_bar_layout_cats(area, cats).into_iter()
          .find(|(_, r)| col >= r.x && col < r.x + r.width && row == r.y)
          .map(|(i, _)| i)
  }

  /// Groups-shaped wrapper over `menu_bar_label_at` — mirrors the existing
  /// `menu_bar_layout` / `menu_bar_layout_cats` pair so the two active-menu mouse arms
  /// need no cats plumbing.
  pub(crate) fn menu_bar_hit(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::menu::MenuRowAction)>)],
      col: u16, row: u16) -> Option<usize>
  {
      let cats: Vec<crate::registry::MenuCategory> = groups.iter().map(|g| g.0).collect();
      menu_bar_label_at(area, &cats, col, row)
  }
  ```

- [ ] **3.4 Impl (mouse migration):** in `mouse.rs::mouse_menu`, replace the `Moved` arm's
  `bar_hit` block:
  ```rust
          let bar_hit: Option<usize> = {
              let groups = &editor.menu.as_ref().unwrap().groups;
              crate::chrome_geom::menu_bar_hit(hit_area, groups, ev.column, ev.row)
          };
  ```
  and the `Down(Left)` arm's `bar_hit` block with the identical three lines. In `mouse.rs::handle`'s
  `CellHit::MenuBar` arm, replace the `cats_hit` layout+find with:
  ```rust
                  let cats_hit = crate::chrome_geom::menu_bar_label_at(
                      area, &crate::registry::MENU_ORDER, ev.column, ev.row);
  ```
  (the `if let Some(order_idx) = cats_hit { editor.menu = Some(crate::menu::empty_at(order_idx)); }`
  consumer and the "row-0 click OFF the labels does nothing" comment stay). The three inline
  `find(|(_, r)| ev.column >= r.x && …)` closures over bar rects are now GONE from mouse.rs —
  `grep -n "menu_bar_layout" wordcartel/src/mouse.rs` (from the repo root) should show no
  remaining production callers (tests may keep theirs).

- [ ] **3.5 Impl (marker paint):** in `render_overlays.rs`: add `Modifier` to the ratatui import
  group (`style::Modifier`), add `menu_bar_marker_col,` to the chrome_geom import list, and append
  AFTER `paint_menu_bar`'s `match editor.menu { … }` block, still inside the fn:
  ```rust
      // B9 §3.4: dim `»` in the bar's last column whenever even the floor rung clips. Same
      // cats source as the arm that just painted, so marker and layout can never disagree.
      let cats: Vec<crate::registry::MenuCategory> = match editor.menu {
          Some(ref menu) if !menu.groups.is_empty() => menu.groups.iter().map(|g| g.0).collect(),
          _ => crate::registry::MENU_ORDER.to_vec(),
      };
      if let Some(mc) = menu_bar_marker_col(menu_area, &cats) {
          frame.render_widget(
              Paragraph::new("»").style(cs.menu_closed.add_modifier(Modifier::DIM)),
              Rect::new(mc, area.y, 1, 1));
      }
  ```

- [ ] **3.6 PINS (green-on-arrival — NOT red-first, and expected to pass immediately):**
  `mouse.rs` tests (reusing `ctx()`, `handle_flat`, `down`, `moved`). These cannot be red
  first: after Task 2 the OLD closures already consume the rung-aware rects, so
  compressed-cell clicks and hovers work through them TODAY — the consolidation in 3.4 is a
  behavior-preserving refactor (H27 `handle_flat`-migration precedent: the existing green
  suite plus these pins are the oracle). Their job is to guard the migration against
  dropping a per-category case, not to drive it. Run them BEFORE 3.4 and confirm they pass
  against the old closures (that confirmation is the point — record it in the task report):
  ```rust
  /// B9 (spec §6c) — regression PIN, green-on-arrival: compressed-cell clicks already work
  /// through the pre-consolidation closures (Task 2 made the shared layout rung-aware); this
  /// pins every category through the migrated inactive-bar arm so the 3.4 refactor cannot
  /// silently drop one. NOT red-first — behavior-preserving refactor (H27 precedent).
  #[test]
  fn narrow_bar_click_opens_every_category_from_its_compressed_cell() {
      use crate::config::MenuBarMode;
      let (reg, ex, clk, tx, km) = ctx();
      for idx in 0..crate::registry::MENU_ORDER.len() {
          let mut e = Editor::new_from_text("hello\n", None, (40, 8));
          crate::derive::rebuild(&mut e);
          e.menu_bar_mode = MenuBarMode::Pinned;
          e.menu = None;
          let area = ratatui::layout::Rect::new(0, 0, 40, 8);
          let bar = crate::chrome_geom::menu_bar_layout_cats(area, &crate::registry::MENU_ORDER);
          let (_, r) = bar.iter().find(|(i, _)| *i == idx).expect("all eight cells laid out");
          handle_flat(&mut e, down(r.x + 1, 0), &reg, &km, &ex, &clk, &tx,
              &crate::test_support::test_fs());
          assert_eq!(e.menu.as_ref().map(|m| m.open), Some(idx),
              "category {idx} must open from its compressed cell");
      }
  }

  /// B9 (spec §6c) — regression PIN, green-on-arrival (same refactor-oracle reasoning):
  /// hover on the ACTIVE compressed bar switches the open category, before and after the
  /// Moved-arm migration.
  #[test]
  fn narrow_bar_hover_switches_categories_on_the_compressed_cells() {
      let (reg, ex, clk, tx, km) = ctx();
      let mut e = Editor::new_from_text("hello\n", None, (40, 8));
      crate::derive::rebuild(&mut e);
      e.menu = Some(crate::menu::empty());
      crate::app::hydrate_overlays(&mut e, &reg, &km);
      let area = ratatui::layout::Rect::new(0, 0, 40, 8);
      let hit_area = crate::chrome_geom::menu_area(area);
      let groups = e.menu.as_ref().unwrap().groups.clone();
      let bar = crate::chrome_geom::menu_bar_layout(hit_area, &groups);
      let (target, r) = bar[bar.len() - 1]; // the last built category's compressed cell
      assert_ne!(e.menu.as_ref().unwrap().open, target, "precondition: not already open");
      handle_flat(&mut e, moved(r.x + 1, 0), &reg, &km, &ex, &clk, &tx,
          &crate::test_support::test_fs());
      assert_eq!(e.menu.as_ref().unwrap().open, target, "hover switched to the hovered cell");
  }
  ```
  (Timing note: these pins reference no new symbols, so they can land with 3.1 and run green
  against the old closures while 3.1's marker tests run red — the contrast IS the TDD record.)

- [ ] **3.7** Task-3 tests green (3.1's marker tests now green; 3.6's pins still green through
  the migrated arms); FULL suite green (the migrated arms must keep every existing mouse/menu
  test passing unmodified — same rects, same semantics on unclipped bars).

- [ ] **3.8** Gates. Commit:
  ```
  group3 T3: B9 — dim » overflow marker + one shared bar hit-test for all three mouse sites

  menu_bar_label_at/menu_bar_hit consolidate the hand-parallel find-closures (H21-style);
  the marker column is derived from the same layout the painter uses, so the visual marker
  and its click-dead column agree by construction (I5).
  ```
  (+ trailers). Ledger line.

### Task 4: Dropdown shift-left-to-fit

- [ ] **4.1 RED:** `chrome_geom.rs` tests (near `tall_menu_groups`):
  ```rust
  /// Eight real categories, each one leaf; the OPEN one gets a long leaf so its natural
  /// dropdown width exceeds the room right of its label.
  #[cfg(test)]
  fn eight_cat_groups(open_leaf: &str)
      -> Vec<(crate::registry::MenuCategory, Vec<(String, crate::menu::MenuRowAction)>)>
  {
      crate::registry::MENU_ORDER.iter().map(|&cat| {
          let label = if cat == crate::registry::MenuCategory::Export { open_leaf.to_string() }
                      else { "item".to_string() };
          (cat, vec![(label, crate::menu::MenuRowAction::Command(crate::registry::CommandId("move_right")))])
      }).collect()
  }

  /// B9 §3.5: a right-edge category's dropdown shifts LEFT to fit at its natural width
  /// instead of clamping into a sliver; on a wide area the anchor is unchanged (no churn).
  #[test]
  fn menu_dropdown_shifts_left_to_fit_instead_of_slivering() {
      let leaf = "a long export item label!!";
      let want_w = leaf.chars().count() as u16 + 2;
      let groups = eight_cat_groups(leaf);
      let open = 7; // Export — the last, right-edge category
      let area = Rect::new(0, 0, 60, 24); // Full rung: Export's label right edge is col 54
      let label_x = menu_bar_layout(area, &groups)[open].1.x;
      let r = menu_dropdown_rect(area, &groups, open).expect("dropdown");
      assert_eq!(r.width, want_w, "natural width, not a sliver");
      assert!(r.x < label_x, "shifted left of its label");
      assert_eq!(r.x + r.width, area.right(), "right edge lands on the area's");
      assert_eq!(menu_dropdown_row_at(area, &groups, open, 0, r.x + 1, r.y), Some(0),
          "hit-test agrees with the shifted box");
      let wide = Rect::new(0, 0, 120, 24);
      let wide_label_x = menu_bar_layout(wide, &groups)[open].1.x;
      let rw = menu_dropdown_rect(wide, &groups, open).expect("dropdown");
      assert_eq!(rw.x, wide_label_x, "anchor unchanged when the natural width fits");
      assert_eq!(rw.width, want_w);
  }
  ```
  Run — MUST FAIL on the `want_w` assertion (the pre-T4 `menu_dropdown_rect` — unchanged by
  Tasks 1–3 — clamps the width to the room right of the label; the cell positions in the
  comments are post-T2 Full-rung facts, which is the tree this task runs on). Quote it.

- [ ] **4.2 GREEN:** rewrite `chrome_geom::menu_dropdown_rect`'s body (signature unchanged):
  ```rust
  pub(crate) fn menu_dropdown_rect(area: Rect, groups: &[(crate::registry::MenuCategory, Vec<(String, crate::menu::MenuRowAction)>)], open: usize) -> Option<Rect> {
      let bar = menu_bar_layout(area, groups);
      let (_, label_rect) = bar.get(open)?;
      let leaves = &groups.get(open)?.1;
      if leaves.is_empty() { return None; }
      let width = (leaves.iter().map(|(l, _)| l.chars().count()).max().unwrap_or(0) as u16 + 2)
          .min(area.width); // never wider than the whole area (B9 §3.5)
      if width == 0 { return None; } // area.width == 0 — nothing to paint into
      let avail_below = area.height.saturating_sub(1) as usize; // rows under the bar
      let list_h = leaves.len().min(15).min(avail_below);
      if list_h == 0 { return None; } // cramped terminal: no room — never paint past the boundary
      // B9 §3.5 shift-left-to-fit: anchor at the label where it fits, else slide left so the
      // box's right edge lands on the area's — the dropdown is ALWAYS fully on-screen (I3),
      // even for a clipped-label category opened by keyboard.
      let x = label_rect.x.min(area.right().saturating_sub(width));
      Some(Rect::new(x, area.y + 1, width, list_h as u16))
  }
  ```
  Bounds argument (from spec §3.5, for the reviewer): `width ≤ area.width` ⇒
  `area.right() − width ≥ area.x`, so `x ≥ area.x` and `x + width ≤ area.right()`.

- [ ] **4.3** FULL suite green — spec §6f predicts zero churn (`menu_dropdown_windows_a_tall_category`,
  `dropdown_indicator_row_hit_test_returns_none`, the mouse drift tests all use left-anchored
  categories where the `min` never bites). Any existing-test red = STOP, report.

- [ ] **4.4** Gates. Commit:
  ```
  group3 T4: B9 — menu_dropdown_rect shifts left to fit instead of slivering at the right edge

  Natural width wherever the area allows; the shared helper moves paint and hit-test
  together (I3: a dropdown is always fully on-screen, even for a clipped-label category).
  ```
  (+ trailers). Ledger line.

### Task 5: Extreme-width belt-and-braces render

- [ ] **5.1** `render.rs` tests (sibling of the prompt-detail template; green-on-arrival is
  expected — it PINS Task 1 + the census against all the new paint code):
  ```rust
  /// H23 belt-and-braces (spec §6d): after the seed widen, no full-render path multiplies
  /// the width — one draw at an absurd hostile-Resize width must neither panic nor resize
  /// the frame. Height 4 keeps the TestBackend cell grid small (~87K cells).
  #[test]
  fn extreme_width_full_render_survives() {
      let mut e = Editor::new_from_text("# Title\n\nbody\n", None, (21846, 4));
      derive::rebuild(&mut e);
      let buf = render_to_buffer(&mut e, 21846, 4);
      assert_eq!((buf.area.width, buf.area.height), (21846, 4));
  }
  ```
  If this test is unexpectedly slow (> a few seconds), shrink height to 2, not width (spec §9.1).

- [ ] **5.2** Full gates. Commit:
  ```
  group3 T5: H23 — extreme-width full-render pin (21846x4)
  ```
  (+ trailers). Ledger line.

### Task 6: Final verification (no merge)

- [ ] **6.1** `cargo test --workspace` — green; `cargo build` + `cargo test --no-run`
  warning-free; `cargo clippy --workspace --all-targets` — clean.
- [ ] **6.2** Run `scripts/smoke/run.sh` and QUOTE its one-line summary verbatim in the pre-merge
  report (mandatory-run, advisory-pass — a red result never blocks, it is surfaced to the human).
- [ ] **6.3** Report branch ready for the two final gates (Fable whole-branch review + Codex
  pre-merge GO/NO-GO). Do NOT merge, do NOT push.
- [ ] **6.4** *Post-merge step (for the controller, not this branch):* flip B9 + H23 to
  `shipped` in `backlog.toml`, move their prose to `docs/backlog-archive.md` (H23's archive note
  corrects the "sweep the sibling helpers" framing to the spec §4.2 census result: ONE u16
  multiply, no siblings), repoint `doc =`, `scripts/backlog bless` — the H31-era pattern of a
  separate backlog commit on main after the merge.

## Self-Review (performed at plan time)

- **Signatures re-verified against the working tree @ `1a994ad`:** `menu_bar_layout_cats` /
  `menu_bar_layout` / `menu_dropdown_rect` / `menu_dropdown_row_at` / `palette_overlay_rect`
  (chrome_geom.rs); `paint_menu_bar`'s two arms and its `menu_area` local (render_overlays.rs);
  `mouse_menu`'s Moved/Down `bar_hit` blocks and `handle`'s `CellHit::MenuBar` arm with its
  `area = Rect::new(0, 0, w, h)` local (mouse.rs); `category_label_pub` (menu.rs);
  `MENU_ORDER` + `MenuCategory` derives `Clone, Copy, PartialEq, Eq, Debug` (registry.rs);
  test helpers `render_to_buffer`/`row_string`/`screen_text` (render.rs tests),
  `ctx`/`handle_flat`/`down`/`moved` (mouse.rs tests), `hydrate_overlays` (app.rs),
  cell-style idiom `buf[(x, y)].style().add_modifier.contains(..)` (render.rs tests).
- **Intermediate greenness:** after Task 2 the mouse closures still consume the SAME
  (now rung-aware) rects via `menu_bar_layout*`, so paint/hit-test agree mid-plan; the marker
  and its dead column arrive together in Task 3.
- **`menu_bar_hit` wrapper** is a plan-level addition beyond the spec's named helpers (spec names
  `menu_bar_label_at`): it mirrors the existing layout/layout_cats groups-vs-cats pair so the two
  active-menu arms need no cats plumbing. No behavior beyond delegation.
- **Existing-test churn:** none predicted (all existing bar/dropdown fixtures are ≥ 62 cols or
  single-category, where FullPadded reproduces today's geometry byte-identically); the plan
  treats any needed edit to an existing test as a STOP-and-report event.
- **Budget:** `render.rs` production +2 physical lines (the Task 1.4 comment) → 899 of the
  900 hub budget; chrome_geom/mouse/render_overlays have no hub budgets; no function
  approaches `too_many_lines` 100 (`paint_menu_bar` grows to ~45 lines).

## Codex plan-gate fold log

- **Round 1 (NO-GO → folded, this revision):** Important — Task 3's hover/click tests were
  labeled red-first but are green after Task 2 (the old closures consume the rung-aware rects);
  restructured Task 3 into genuinely-red marker tests (3.1: the `»` glyph does not paint today,
  and — ON THE POST-T2 TREE where 3.1 runs, before the T3 migration — a click on the marker
  column opens the covering clipped category through the un-migrated `CellHit::MenuBar` arm:
  Exp at 35 cols, View at 20 [Short-rung ranges; see the Round 2 entry — these are POST-T2 facts,
  not current-main facts]) + green-on-arrival
  regression PINS for the behavior-preserving closure consolidation (3.6, H27 precedent
  language, run-before-migrate confirmation required). Minor — `render.rs` budget arithmetic
  corrected to 897 → 899/900 (the census comment is two physical lines); grep verification
  path corrected to `wordcartel/src/mouse.rs` from the repo root. All other plan content was
  confirmed GO (T1 panic real; T2 staged-red reasoning sound; T4 paint/hit agreement sound).
- **Round 2 (NO-GO → folded, this revision):** one wording issue — T3.1's red-baseline prose
  attributed the Short-rung cell ranges (`Exp` [32, 36) at 35 cols, `View` [18, 23) at 20 cols)
  to the "current source"; those are POST-T2 facts (the pre-plan full-label bar puts col 34 in
  `Documents` [33, 44) and col 19 in `Format` [19, 27)). Fixed by naming the baseline task-state
  everywhere: 3.1 is red AS SEQUENCED — introduced and run on the post-T2 tree, before the T3
  migration/paint, and its red-ness depends on T2's rung-aware layout (do not cherry-pick onto
  an earlier tree). Same baseline-naming applied to 3.1's render-test doc comment and T4.1's
  run-note. Test logic, ranges, and outcomes unchanged (re-verified: post-T2 Short cells at
  w=35 put Exp at [32, 36) → un-migrated closure opens Export from col 34; at w=20 View's
  [18, 23) covers col 19). All other content reconfirmed GO by the gate.
