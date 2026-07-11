# E7 — Review render mode: implementation plan

**Date:** 2026-07-11
**Spec:** `docs/superpowers/specs/2026-07-11-e7-review-render-mode-design.md` (GO — Codex-gated)
**Grounded against:** `main` @ HEAD (settled tree). Every symbol/signature below was grep/LSP-verified;
anchors are symbol NAMES — re-locate by name, not line number (lines drift as tasks edit).
**Do NOT run `cargo fmt`.** Match house style by hand (dense, 4-space, ~100-col, em-dash `—`, no emoji).

## How to use this plan
Seven tasks in dependency order. Each is independently dispatchable to a fresh implementer and is
**TDD**: write the named failing test first (RED), implement, confirm green, commit. Every commit ends
with the two project trailers (`Co-Authored-By` + `Claude-Session`). Do not combine tasks. After each
task: `cargo test -p wordcartel -p wordcartel-core` + `cargo clippy --workspace --all-targets` must be
clean for the crate(s) touched (both are merge GATEs).

**Dependency graph:** T1 → T2 → T3 → T4 → T5; T6 depends on T3+T4; T7 is the final gate sweep.
T3 and T4 both depend on T2 (they call `should_run_diagnostics`); do T3 before T4.

**Command-surface contract conformance (restated at plan level — `docs/design/command-surface-contract.md`):**
- **Law 6 / rule 8 (one setter, multi-state = primitives + stateful representative):** T4 adds the single
  `Editor::set_render_mode` setter; the four `view_*` primitives (`menu: None`) and the promoted stateful
  `cycle_render_mode` all route through it. Template copied verbatim from `scrollbar_off/auto/on` +
  `cycle_scrollbar`.
- **Law 3 (palette exhaustive):** the four primitives + cycle are ordinary registry rows; `palette_is_exhaustive_over_the_registry` stays green (T4/T7).
- **Law 7 (hints re-resolve):** T5 binds `view_review` in BOTH presets; `hints_reresolve_on_preset_switch`,
  `custom_bind_surfaces_in_menu_and_palette`, `both_presets_resolve_against_builtins` stay green (T5/T7).
- **Law 2:** render mode is per-buffer runtime view state, not a persisted `SettingsSnapshot` field —
  `every_persisted_setting_has_a_command` is structurally unaffected (no new persisted key).
- **Merge-GATE invariant tests every task must keep green:** `palette_is_exhaustive_over_the_registry`
  (`palette.rs`), `every_persisted_setting_has_a_command` (`settings.rs`), `hints_reresolve_on_preset_switch`
  + `both_presets_resolve_against_builtins` (`keymap.rs`), `custom_bind_surfaces_in_menu_and_palette`
  (`menu.rs`); plus `cargo test`, workspace clippy, and the `module_budgets` hub-budget test
  (`app_rs_stays_a_thin_dispatch_hub`, budget 1000 production lines) and `clippy::too_many_lines` (100).

## Existing-test migration inventory (complete — the draft-quiet flip breaks default-LivePreview tests)
The Review-only compute/display/action gates break every existing test that, in default LivePreview,
dispatches/wakes diagnostics, paints seeded diagnostics, exercises `quick_fix`/`diag_next`/`diag_prev`, or
cycles render mode expecting a specific mode. Swept via four `rg` passes (cycle-mode asserts; arm/dispatch/
deadline; seed+underline; the three diag commands). **14 tests migrate; each is owned by the task that
breaks it and re-detailed there.** The gate can confirm coverage against this table:

| Test | File | Owner | Migration |
|---|---|---|---|
| `cycle_render_mode_rotates_through_modes` | commands.rs | T1 | 4-state order (Live→Review→SRC-HI→Source) |
| `source_highlighted_makes_inactive_heading_show_raw` | commands.rs | T1 | cycle twice to reach SourceHighlighted |
| `tick_dispatches_a_due_check_once` | app.rs | T2 | set Review |
| `gated_subsystems_yield_none` | timers.rs | T2 | set Review before the diagnostics sub-block |
| `diagnostics_underline_the_flagged_glyphs` | render.rs | T2 | set Review |
| `stale_diagnostics_are_not_painted` | render.rs | T2 | set Review (isolates version-staleness) |
| `default_search_and_diag_unchanged` | render.rs | T2 | set Review in the diagnostic sub-block |
| `diagnostics_probe` | e2e.rs | T2 | set Review **and** `diag_cfg.enabled = true` |
| `quick_fix_applies_suggestion_as_undoable_edit` | app.rs | T4 | set Review |
| `quick_fix_on_stale_diagnostics_is_noop_no_overlay` | app.rs | T4 | set Review (isolates stale guard) |
| `diag_next_prev_move_caret_with_wrap` | app.rs | T4 | set Review |
| `quick_fix_refuses_stale_apply_after_concurrent_edit` | app.rs | T4 | set Review |
| `diag_next_into_fold_auto_unfolds` | registry.rs | T4 | set Review before `dispatch_id` |
| `diag_prev_into_fold_auto_unfolds` | registry.rs | T4 | set Review before `dispatch_id` |

**Reviewed and deliberately NOT migrated (rationale in the owning task):** `cycle_render_mode_keeps_caret_visible`
(asserts caret layout, not mode); `diag_deadline_excluded_when_in_flight` + `settled_editor_arms_no_deadline`
(inline the old gating logic against a raw `DiagStore` — never call the real gated fn); `next_wake_none_when_settled`
(unarmed); the `save.rs` late-`DiagnosticsDone` tests (result-landing not gated); `diagnostics_run.rs` internal
`DiagStore` tests; `open_diag_clears_siblings…` + the two `mouse.rs` diag-click tests (use `open_diag`/
`diag_apply_selected` directly, bypassing the gated commands).

---

## Task 1 — `RenderMode::Review` variant + compiler-forced render sites

**Goal:** add the fourth variant and satisfy every site the compiler forces, so the tree builds green.
Review renders exactly like LivePreview (spec §1.1, §1.2). No diagnostics/gating/command-surface work here.

**Files:** `wordcartel/src/editor.rs`, `wordcartel/src/lines.rs`, `wordcartel/src/render_status.rs`,
`wordcartel/src/commands.rs`, `wordcartel/src/render.rs` (test only).

### TDD — RED first
Add these tests (they fail to compile until the variant exists, then fail on assertions until the arms are added):

1. `wordcartel/src/lines.rs` `#[cfg(test)] mod tests` (add if absent; `use super::*`):
```rust
#[test]
fn review_mode_mirrors_live_preview() {
    use crate::editor::RenderMode;
    use wordcartel_core::style::LineRender;
    assert_eq!(line_render_for(RenderMode::Review, true),  LineRender::RawPlain);
    assert_eq!(line_render_for(RenderMode::Review, false), LineRender::Concealed);
}
```
2. `wordcartel/src/render_status.rs` tests:
```rust
#[test]
fn status_line_shows_review_label() {
    let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
    e.active_mut().view.mode = crate::editor::RenderMode::Review;
    assert!(status_left_text(&e).contains("[REVIEW]"), "review mode labels [REVIEW]");
}
```
3. `wordcartel/src/render.rs` tests (the `plain_source` pinning guard — Review must NOT be plain-source):
```rust
#[test]
fn review_is_not_plain_source() {
    use crate::editor::RenderMode;
    let mut ed = Editor::new_from_text("# Title\n", None, (40, 6));
    ed.active_mut().view.mode = RenderMode::Review;
    crate::derive::rebuild(&mut ed);
    let review = render_to_buffer(&mut ed, 40, 6);
    ed.active_mut().view.mode = RenderMode::LivePreview;
    crate::derive::rebuild(&mut ed);
    let live = render_to_buffer(&mut ed, 40, 6);
    ed.active_mut().view.mode = RenderMode::SourcePlain;
    crate::derive::rebuild(&mut ed);
    let plain = render_to_buffer(&mut ed, 40, 6);
    assert_eq!(review, live,  "Review renders styled exactly like LivePreview");
    assert_ne!(review, plain, "Review is NOT raw-plain like SourcePlain");
}
```
*(`render_to_buffer` is the real helper in `render.rs`'s test module (returns
`ratatui::buffer::Buffer`, which implements `PartialEq`), so buffer-equality is a clean styled≡LivePreview
pin without needing a stringify helper. `# Title` is a styled heading, so LivePreview and SourcePlain
render measurably differently — the `assert_ne` is meaningful.)*

### GREEN — implementation
1. **`editor.rs` — the variant.** Extend the enum (keep existing derives; the surrounding block is the
   `#[derive(Clone, Copy, PartialEq, Eq, Debug)] pub enum RenderMode`):
```rust
pub enum RenderMode {
    LivePreview,
    SourceHighlighted,
    SourcePlain,
    Review,
}
```
2. **`lines.rs::line_render_for`** — combine Review into the LivePreview arm and update the doc comment
   to name all four modes:
```rust
match mode {
    LivePreview | Review => if is_active_line { RawPlain } else { Concealed },
    SourceHighlighted    => RawStyled,
    SourcePlain          => RawPlain,
}
```
3. **`render_status.rs::status_left_text`** — add the label arm to the `[MODE]` match:
```rust
crate::editor::RenderMode::Review => "REVIEW",
```
4. **`commands.rs::Command::CycleRenderMode`** — the current 3-arm `match editor.active().view.mode`
   is non-exhaustive once `Review` exists (compile error). Set the **final** cycle order now
   (Live→Review→SRC-HI→Source→Live, spec §3.3), still writing the field directly (T4 will refactor this
   arm to route through `set_render_mode`):
```rust
Command::CycleRenderMode => {
    editor.active_mut().view.mode = match editor.active().view.mode {
        RenderMode::LivePreview       => RenderMode::Review,
        RenderMode::Review            => RenderMode::SourceHighlighted,
        RenderMode::SourceHighlighted => RenderMode::SourcePlain,
        RenderMode::SourcePlain       => RenderMode::LivePreview,
    };
    derive::rebuild(editor);
    nav::ensure_visible(editor); // a mode change can alter layout/scroll
    CommandResult::Handled
}
```
6. **`derive.rs::LayoutKey.mode` / `nav.rs::layout_line_on_demand`** — NO change (they consume
   `line_render_for`/the `Copy+Eq` mode automatically; spec §1.2 sites 4–5). Do not touch.

### Existing-test migrations owned by T1 (the new cycle order breaks single-cycle mode assertions)
Found via `rg -n "CycleRenderMode|cycle_render_mode" wordcartel/src -g '*.rs' | rg "assert|run\("`.
- **`commands.rs::cycle_render_mode_rotates_through_modes`** — update to the 4-state order + its doc
  comment: after cycle 1 assert `Review`, then `SourceHighlighted`, then `SourcePlain`, then `LivePreview`
  (four `run(Command::CycleRenderMode …)` steps).
- **`commands.rs::source_highlighted_makes_inactive_heading_show_raw`** — it does ONE
  `run(Command::CycleRenderMode, …)` expecting `SourceHighlighted`; the new order lands that single cycle
  on **Review**. Fix by cycling **twice** (Live→Review→SRC-HI) before the `assert_eq!(…, SourceHighlighted)`
  and the raw-marker layout assertion:
```rust
run(Command::CycleRenderMode, &mut e, &clk);   // Live → Review
run(Command::CycleRenderMode, &mut e, &clk);   // Review → SourceHighlighted
assert_eq!(e.active().view.mode, RenderMode::SourceHighlighted);
```
  (Alternative: `e.active_mut().view.mode = RenderMode::SourceHighlighted; derive::rebuild(&mut e);` — the
  test's intent is SourceHighlighted *rendering*, not the cycle path. Cycle-twice is chosen to keep
  exercising the command.)
- **Reviewed, NO change:** `commands.rs::cycle_render_mode_keeps_caret_visible` asserts the caret's line is
  laid out after one cycle (now landing on Review) — Review mirrors LivePreview layout, so the caret line
  is still laid out; the assertion holds. Leave it (do not add a mode assertion).

**Verify:** `cargo build -p wordcartel` warning-free; the three RED tests + the updated rotate test green.

**Commit:** `E7 T1: add RenderMode::Review (mirrors LivePreview) + compiler-forced render sites`

---

## Task 2 — Diagnostics gating helpers + gate the compute/display sites (except the reduce seam)

**Goal:** introduce `should_run_diagnostics`/`should_show_diagnostics` and apply them at every gate site
EXCEPT the `app.rs::reduce` edit-arm (that is T3's seam). Flip diagnostics to Review-only for dispatch,
wake, recheck, the class-B re-arms, and display. (Spec §2.1, §2.2 items 2/3/4/5, §2.3.)

**Files:** `wordcartel/src/diagnostics_run.rs`, `wordcartel/src/timers.rs`, `wordcartel/src/registry.rs`,
`wordcartel/src/search_ui.rs`, `wordcartel/src/render.rs`, `wordcartel/src/e2e.rs`.

### TDD — RED first
1. `diagnostics_run.rs` tests (truth table):
```rust
#[test]
fn should_run_diagnostics_only_in_review_and_enabled() {
    use crate::editor::{Editor, RenderMode};
    let mut e = Editor::new_from_text("x\n", None, (40, 10));
    e.diag_cfg.enabled = true;
    for (mode, want) in [(RenderMode::LivePreview, false), (RenderMode::Review, true),
                         (RenderMode::SourceHighlighted, false), (RenderMode::SourcePlain, false)] {
        e.active_mut().view.mode = mode;
        assert_eq!(should_run_diagnostics(&e), want, "{mode:?} enabled");
        assert_eq!(should_show_diagnostics(&e), want, "show mirrors run: {mode:?}");
    }
    e.active_mut().view.mode = RenderMode::Review;
    e.diag_cfg.enabled = false;
    assert!(!should_run_diagnostics(&e), "disabled → false even in Review");
}
```
2. `timers.rs` tests (the spin-class / idle-is-free guardrail for the `diag_deadline` gate, spec §2.2 site 5 / §8.1):
```rust
#[test]
fn armed_diag_deadline_is_none_outside_review() {
    use crate::editor::RenderMode;
    let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
    e.diag_cfg.enabled = true;
    e.active_mut().view.mode = RenderMode::LivePreview;
    e.active_mut().diagnostics.arm(0, 400); // recheck_due_at = Some(400), in_flight None
    assert_eq!(diag_deadline(&e, 10_000), None, "no wake for a non-Review armed store (no spin)");
    e.active_mut().view.mode = RenderMode::Review;
    assert_eq!(diag_deadline(&e, 10_000), Some(400), "Review: the armed deadline is live");
}
```
3. `render.rs` tests (display gate on leaving Review):
```rust
#[test]
fn diagnostics_paint_only_in_review() {
    use crate::editor::{Editor, RenderMode};
    let mut e = Editor::new_from_text("teh cat\n", None, (40, 6));
    let v = e.active().document.version;
    e.active_mut().diagnostics.diagnostics = vec![wordcartel_core::diagnostics::Diagnostic {
        range: 0..3, kind: wordcartel_core::diagnostics::DiagnosticKind::Spelling,
        message: "x".into(), suggestions: vec![] }];
    e.active_mut().diagnostics.computed_version = v;
    e.active_mut().view.mode = RenderMode::Review;
    crate::derive::rebuild(&mut e);
    assert!(row_has_underline(&render_to_buffer(&mut e, 40, 6), 0), "painted in Review");
    e.active_mut().view.mode = RenderMode::LivePreview;
    crate::derive::rebuild(&mut e);
    assert!(!row_has_underline(&render_to_buffer(&mut e, 40, 6), 0), "hidden the instant we leave Review");
}
```
4. `registry.rs` test (recheck is Review-only):
```rust
#[test]
fn recheck_diagnostics_arms_only_in_review() {
    // Build an Editor in Review with diag enabled; dispatch "recheck_diagnostics";
    // assert recheck_due_at becomes Some. Repeat in LivePreview; assert it stays None.
    // (Use the existing dispatch_id/registry test harness in this module.)
}
```

### GREEN — implementation
1. **`diagnostics_run.rs`** — add the two helpers beside `DiagStore`/`diag_due` (module already has
   `use crate::editor::{BufferId, Editor};`):
```rust
/// Compute gate: diagnostics arm/dispatch only when the feature is enabled AND the active buffer
/// is in the Review render mode. (Spec §2.1.)
pub fn should_run_diagnostics(editor: &Editor) -> bool {
    editor.diag_cfg.enabled && editor.active().view.mode == crate::editor::RenderMode::Review
}
/// Display gate: underlines paint under exactly the same predicate. Distinct name for the distinct
/// role (compute vs paint); delegates so the two cannot drift.
pub fn should_show_diagnostics(editor: &Editor) -> bool { should_run_diagnostics(editor) }
```
2. **`timers.rs::on_tick`** — the dispatch guard (spec §2.2 item 2):
```rust
if crate::diagnostics_run::should_run_diagnostics(editor)
    && crate::diagnostics_run::diag_due(&editor.active().diagnostics, now, version)
{ … }
```
3. **`timers.rs::diag_deadline`** — the wake gate (spec §2.2 item 5 — the spin fix):
```rust
fn diag_deadline(e: &Editor, _now: u64) -> Option<u64> {
    if crate::diagnostics_run::should_run_diagnostics(e)
        && e.active().diagnostics.in_flight_version.is_none()
    { e.active().diagnostics.recheck_due_at } else { None }
}
```
4. **`registry.rs::recheck_diagnostics` handler** (spec §2.2 item 3):
```rust
r.register("recheck_diagnostics", "Recheck Diagnostics", None, |c| {
    if crate::diagnostics_run::should_run_diagnostics(c.editor) {
        c.editor.active_mut().diagnostics.arm(c.clock.now_ms(), 0);
    }
    CommandResult::Handled
});
```
5. **`search_ui.rs::diag_apply_selected`** — re-gate the TWO existing class-B re-arms (ignore + add-dict
   branches) from `if editor.diag_cfg.enabled` to `if crate::diagnostics_run::should_run_diagnostics(editor)`
   (spec §2.2 item 4). These re-arms STAY (they cover the non-`version` ignore-set/dictionary change).
   The suggestion-fix branch and `search_replace_all`/`search_step_apply`/`search_step_rest` get **no**
   re-arm — the T3 seam covers them (they bump `document.version`). Leave those functions untouched here.
6. **`render.rs::gather_row_ctx`** — add the display gate to `diag_active` (spec §2.3):
```rust
let diag_active = crate::diagnostics_run::should_show_diagnostics(editor)
    && editor.active().diagnostics.valid_for(editor.active().document.version);
```
### Existing-test migrations owned by T2 (draft-quiet breaks default-LivePreview diag dispatch/wake/paint)
Found via `rg -n "diagnostics\.arm|diag_due|diag_deadline|DiagnosticsDone|on_tick|row_has_underline|computed_version" wordcartel/src -g '*.rs'`.
For each, the fix is `e.active_mut().view.mode = crate::editor::RenderMode::Review;` at setup (diagnostics
are default-`enabled=true` via `Editor::new_from_text`, so Review alone suffices UNLESS the harness disabled
them — noted per item).
- **`app.rs::tick_dispatches_a_due_check_once`** — arms + drives `Msg::Tick` expecting `in_flight_version`
  set; `on_tick` is now Review-gated. Set Review (the test already sets `diag_cfg.enabled = true`).
- **`timers.rs::gated_subsystems_yield_none`** — the diagnostics sub-block un-gates `in_flight_version`
  and asserts `(diag_deadline)(&e, 10_000) == Some(0)`; with the new `should_run_diagnostics` gate in
  `diag_deadline`, a default-LivePreview buffer returns `None`, failing that assertion. Set Review **before**
  the diagnostics sub-block (its `recheck_due_at`/`in_flight` arming) so the un-gated case still yields
  `Some(0)`; the "None while in-flight" assertion above it still holds (in-flight gate). The reconcile/swap
  sub-blocks are mode-independent — unchanged.
- **`render.rs::diagnostics_underline_the_flagged_glyphs`** — seeds valid diagnostics, asserts
  `row_has_underline`; the display gate hides them in LivePreview. Set Review before `rebuild`.
- **`render.rs::stale_diagnostics_are_not_painted`** — asserts NO underline (version mismatch). Set Review
  before `rebuild` so it still isolates the *version-staleness* guard rather than passing because the mode
  gate hid the underline (spec §7.2).
- **`render.rs::default_search_and_diag_unchanged`** — a two-part test; its **diagnostic sub-block** (the
  `{ … row_has_underline(&buf, 0), "Default: diag underline still present" … }` block) seeds valid
  diagnostics in default LivePreview and asserts the underline paints — the display gate now hides it. Set
  Review inside that sub-block before `rebuild`. The **search sub-block** (highlights) is mode-independent —
  leave it untouched.
- **`e2e.rs::diagnostics_probe`** — the `Harness` seeds `diag_cfg.enabled = false` (e2e.rs ~69/85), so this
  probe needs **BOTH** `editor.diag_cfg.enabled = true` **AND** `editor.active_mut().view.mode = RenderMode::Review;`
  on the harness editor before the measured `step_timed(Msg::DiagnosticsDone …)` render, or it measures the
  empty (un-placed) path (spec §7.4). Locate via `rg -n "fn diagnostics_probe" e2e.rs`.
- **Reviewed, NO change (they do not exercise the real gated code):**
  - `app.rs::diag_deadline_excluded_when_in_flight` and `app.rs::settled_editor_arms_no_deadline` — both
    **inline** the old gating logic (`if store.in_flight_version.is_none() { store.recheck_due_at } else { None }`)
    against a raw `DiagStore`/settled buffer; they never call the real (now Review-gated) `timers::diag_deadline`,
    so E7 doesn't touch them. (If a future task re-expresses them via `timers::next_wake`, they'd need Review —
    not in scope here.)
  - `timers.rs::next_wake_none_when_settled` — a clean, unarmed buffer yields `None` regardless of the mode
    gate. Unchanged.
  - `save.rs` late-`DiagnosticsDone` tests (`…must be discarded…`) — exercise `apply_diagnostics_done`'s
    version gate, which E7 does NOT gate (result-landing stays version-only, spec §2.2). Unchanged.
  - `diagnostics_run.rs` internal tests (`arm`/`diag_due`/`valid_for`) — pure `DiagStore`, no mode. Unchanged.

**Note (intermediate state):** the `app.rs::reduce` epilogue arm is deliberately NOT touched in T2 — T3
owns it. After T2, an edit in LivePreview may still set `recheck_due_at` via the old epilogue, but
`on_tick` won't dispatch and `diag_deadline` won't wake (both now Review-gated), so no spin and no
dispatch — a consistent intermediate.

**Verify:** the 4 RED tests green; ALL six T2 migrations green (tick-dispatch, gated_subsystems,
the two paint tests, default_search_and_diag, e2e probe); `cargo test -p wordcartel` green;
clippy clean.

**Commit:** `E7 T2: gate diagnostics dispatch/wake/recheck/display to Review (draft-quiet)`

---

## Task 3 — The unified re-arm seam (`reduce`/`reduce_dispatch` + `arm_if_edited`)

**Goal:** replace the interceptor-bypassing epilogue arm with ONE `(active BufferId, document.version)`-keyed
seam wrapping all of `reduce`, so every ACTIVE-buffer edit — direct, overlay, or prompt-held job — re-arms
exactly once, and a buffer switch never falsely arms. Behavior-preserving extraction that THINS `reduce`
(module-structure GATE). (Spec §2.2 item 1, §8.3.)

**Files:** `wordcartel/src/diagnostics_run.rs`, `wordcartel/src/app.rs`.

### TDD — RED first
1. `diagnostics_run.rs` (`arm_if_edited` unit — spec §7.2):
```rust
#[test]
fn arm_if_edited_arms_only_on_active_buffer_edit_in_review() {
    use crate::editor::{Editor, RenderMode};
    let mut e = Editor::new_from_text("x\n", None, (40, 10));
    e.diag_cfg.enabled = true;
    e.active_mut().view.mode = RenderMode::Review;
    use crate::test_support::TestClock;
    let id = e.active().id;
    let v = e.active().document.version;
    // no version change → no arm
    arm_if_edited(&mut e, id, v, &TestClock(100));
    assert_eq!(e.active().diagnostics.recheck_due_at, None, "equal version: no arm");
    // version increased, same buffer, Review, enabled → arm at now+debounce
    e.active_mut().document.version += 1;
    arm_if_edited(&mut e, id, v, &TestClock(100));
    assert_eq!(e.active().diagnostics.recheck_due_at, Some(100 + e.diag_cfg.debounce_ms));
    // same edit but in LivePreview → no arm
    e.active_mut().diagnostics.recheck_due_at = None;
    e.active_mut().view.mode = RenderMode::LivePreview;
    arm_if_edited(&mut e, id, v, &TestClock(200));
    assert_eq!(e.active().diagnostics.recheck_due_at, None, "not Review: no arm");
    // buffer-identity guard: active id != before_id → no arm even with a version delta
    e.active_mut().view.mode = RenderMode::Review;
    let other = crate::editor::BufferId(id.0.wrapping_add(999));
    arm_if_edited(&mut e, other, v, &TestClock(300));
    assert_eq!(e.active().diagnostics.recheck_due_at, None, "switch (id changed): no arm");
}
```
*(`crate::test_support::TestClock(u64)` is the canonical `#[cfg(test)]` clock implementing
`history::Clock` — use it, NOT `editor.rs`'s module-local `Cell`-based `TestClock`. `diagnostics_run.rs`
has no test module today; add `#[cfg(test)] mod tests { use super::*; … }`.)*

2. `app.rs` tests (seam through `reduce`; use the existing `reduce` test harness — locate a current
   `reduce`-driving test to copy the `Registry`/`KeyTrie`/`Executor`/`Clock`/`mpsc` setup):
```rust
#[test]
fn active_edit_in_review_arms_via_reduce() {
    // Arrange: editor in Review, diag enabled, recheck_due_at cleared.
    // Act: drive one text-insert Msg::Input(key) through reduce().
    // Assert: recheck_due_at == Some(now + debounce_ms).
}
#[test]
fn buffer_switch_in_review_does_not_arm_via_reduce() {
    // Arrange: two buffers A(v0) and B(v1), both Review, active = A, B.recheck_due_at = None.
    // Act: dispatch the switch-to-B path through reduce (workspace::switch_to via a buffer-row/command).
    // Assert: the now-active B has recheck_due_at == None (switch ≠ edit).
}
```

### GREEN — implementation
1. **`diagnostics_run.rs`** — add the seam helper beside `should_run_diagnostics`:
```rust
/// The single diagnostics re-arm seam (spec §2.2 item 1). After a `reduce` message, if the SAME
/// buffer is still active AND its document.version advanced since the pre-dispatch snapshot, arm the
/// debounced recheck — but only when in Review with checking enabled. Wraps every `reduce` exit path
/// (interceptor early-returns AND the normal tail), so every active-buffer edit re-arms exactly once,
/// with no per-path enumeration, no double-arm, and no false arm on a buffer switch (§2.3).
pub fn arm_if_edited(editor: &mut Editor, before_id: BufferId, before_version: u64,
    clock: &dyn wordcartel_core::history::Clock) {
    if editor.active().id == before_id
        && editor.active().document.version != before_version
        && should_run_diagnostics(editor)
    {
        let debounce_ms = editor.diag_cfg.debounce_ms;
        editor.active_mut().diagnostics.arm(clock.now_ms(), debounce_ms);
    }
}
```
2. **`app.rs::reduce`** — extract the dispatch body into a private `reduce_dispatch` and wrap. The current
   `reduce` is: (a) the `#[cfg(debug_assertions)]` F12 smoke-panic trigger, (b) the eleven `X::intercept`
   stages each `Handled::Done(k) => return k`, (c) `let before = editor.active().document.version;`, (d)
   the `match msg { … }`, (e) the epilogue `if version != before { last_edit_at; if diag_cfg.enabled { arm } }`
   + `for o in ex.drain() {…}` + `!editor.quit`. Restructure to:
```rust
pub fn reduce(msg: Msg, editor: &mut Editor, reg: &Registry, keymap: &crate::keymap::KeyTrie,
    ex: &dyn Executor, clock: &dyn Clock, msg_tx: &std::sync::mpsc::Sender<Msg>) -> bool {
    // (the #[cfg(debug_assertions)] F12 smoke-panic trigger stays the FIRST statement, unchanged)
    let before_id = editor.active().id;
    let before_version = editor.active().document.version;
    let keep = reduce_dispatch(msg, editor, reg, keymap, ex, clock, msg_tx);
    crate::diagnostics_run::arm_if_edited(editor, before_id, before_version, clock);
    keep
}

/// The interceptor chain + message match, extracted from `reduce` so the single `arm_if_edited`
/// seam in `reduce` wraps every exit path (H1 dispatch-hub discipline). Behavior-identical to the
/// pre-E7 body minus the inline diagnostics arm (now `arm_if_edited`).
fn reduce_dispatch(msg: Msg, editor: &mut Editor, reg: &Registry, keymap: &crate::keymap::KeyTrie,
    ex: &dyn Executor, clock: &dyn Clock, msg_tx: &std::sync::mpsc::Sender<Msg>) -> bool {
    let msg = match crate::splash::intercept(msg, editor, ex, clock, msg_tx) {
        crate::app::Handled::Done(k) => return k, crate::app::Handled::Pass(m) => m };
    // … the other ten intercept stages, verbatim …
    let before = editor.active().document.version;   // post-interceptor; feeds last_edit_at only
    match msg { /* … unchanged arms … */ }
    if editor.active().document.version != before {
        editor.active_mut().last_edit_at = Some(clock.now_ms());
        // NOTE: the old `if editor.diag_cfg.enabled { …arm… }` block is REMOVED — arm_if_edited
        // (called from reduce, keyed on active id + version) subsumes it.
    }
    for o in ex.drain() { crate::jobs_apply::apply_job_outcome(o, editor, ex, clock, msg_tx); }
    !editor.quit
}
```
   - Move the F12 panic trigger so it is the first statement of `reduce` (before the snapshot). The
     `before` inside `reduce_dispatch` is retained ONLY for `last_edit_at` (normal-path swap timing,
     unchanged — spec §2.2 item 1). Delete only the `if diag_cfg.enabled { arm }` lines.
   - `reduce_dispatch` is private; the public `reduce` signature is UNCHANGED, so the `run()` loop and all
     external callers are untouched (verify: `grep -n "reduce(" wordcartel/src/app.rs` — the run loop call
     site keeps working).
3. **Module budgets.** The split reduces the largest function and adds ~10 lines to `app.rs`. Verify
   `cargo test -p wordcartel module_budgets::app_rs_stays_a_thin_dispatch_hub` stays green and that neither
   `reduce` nor `reduce_dispatch` trips `clippy::too_many_lines` (100). `reduce_dispatch` should land ~80
   production lines; if it exceeds 100, split an intercept sub-group into a helper OR add
   `#[allow(clippy::too_many_lines)]` with a one-line reason ("flat interceptor dispatch table") — do NOT
   raise a budget.
4. **Migration audit.** `grep -n "recheck_due_at\|\.arm(" wordcartel/src/app.rs` inside `#[cfg(test)]` and
   fix any test that assumed the epilogue armed diagnostics on a **default-mode** edit — such a test must
   now put the buffer in Review first (the seam is Review-gated). Confirm `save_and_quit_command_on_unnamed_buffer_does_not_arm`
   still holds (a save/quit is not an edit → no version delta → no arm, unchanged).

**Verify:** RED tests green; full `cargo test -p wordcartel` green; clippy clean; module-budget test green.

**Commit:** `E7 T3: unify diagnostics re-arm into one (buffer-id,version) seam over reduce`

---

## Task 4 — Command surface: shared setter, `view_*` primitives, stateful cycle, Review-only actions

**Goal:** add `Editor::set_render_mode` (law 6), four `view_*` primitives, promote `cycle_render_mode` to
stateful (rule 8), route `CycleRenderMode` through the setter, and make `quick_fix`/`diag_next`/`diag_prev`
Review-only (spec §2.5, §3.1–3.5). All new rows BEFORE `save_settings`.

**Files:** `wordcartel/src/editor.rs`, `wordcartel/src/commands.rs`, `wordcartel/src/registry.rs`.

### TDD — RED first
1. `registry.rs` tests (use the module's existing `dispatch_id`/builtins harness):
```rust
#[test]
fn view_review_command_enters_review_and_arms() {
    // Build editor in LivePreview, diag enabled; dispatch "view_review".
    // Assert active().view.mode == Review AND recheck_due_at == Some(now) (arm-on-enter, debounce 0).
}
#[test]
fn cycle_render_mode_state_label_tracks_mode() {
    // For each mode, set it and assert the registered state fn yields MenuMark::Value(expected)
    // ("Live"/"Review"/"SRC-HI"/"Source").
}
#[test]
fn diag_actions_are_review_only() {
    // With valid stored diagnostics under the caret in LivePreview: quick_fix sets status
    // "no diagnostic here" and leaves editor.diag == None; diag_next/diag_prev leave selection
    // unchanged. In Review the same setup opens the overlay / moves the caret.
}
```
2. `editor.rs` test:
```rust
#[test]
fn set_render_mode_arms_on_enter_review_only() {
    let mut e = Editor::new_from_text("x\n", None, (80, 24));
    e.diag_cfg.enabled = true;
    e.set_render_mode(RenderMode::Review, 500);
    assert_eq!(e.active().diagnostics.recheck_due_at, Some(500), "arm-on-enter at debounce 0");
    e.active_mut().diagnostics.recheck_due_at = None;
    e.set_render_mode(RenderMode::LivePreview, 600);
    assert_eq!(e.active().diagnostics.recheck_due_at, None, "leaving Review never arms");
}
```

### GREEN — implementation
1. **`editor.rs::set_render_mode`** — beside `set_scrollbar_mode`/`set_clipboard_provider` (spec §3.1):
```rust
/// The single setter for the render mode (command-surface contract law 6). All render-mode mutation
/// — the four view_* primitives, the cycle, and any future profile/plugin — routes here. `now_ms`
/// feeds the arm-on-entering-Review debounce timestamp.
pub fn set_render_mode(&mut self, mode: RenderMode, now_ms: u64) {
    self.active_mut().view.mode = mode;
    crate::derive::rebuild(self);
    crate::nav::ensure_visible(self);
    if crate::diagnostics_run::should_run_diagnostics(self) {
        self.active_mut().diagnostics.arm(now_ms, 0);
    }
}
```
2. **`commands.rs::Command::CycleRenderMode`** — route through the setter (order already final from T1);
   the setter owns `rebuild`/`ensure_visible`/arm-on-enter:
```rust
Command::CycleRenderMode => {
    let next = match editor.active().view.mode {
        RenderMode::LivePreview       => RenderMode::Review,
        RenderMode::Review            => RenderMode::SourceHighlighted,
        RenderMode::SourceHighlighted => RenderMode::SourcePlain,
        RenderMode::SourcePlain       => RenderMode::LivePreview,
    };
    editor.set_render_mode(next, clock.now_ms());
    CommandResult::Handled
}
```
   (`run(cmd, editor, clock)` has `clock`; the registry wrapper `fn run(ctx,cmd)` already forwards
   `ctx.clock`. The `cycle_render_mode_rotates_through_modes` test uses a `TestClock` and checks mode
   only — still green.)
3. **`registry.rs`** — at the `cycle_render_mode` registration (the "View menu" line), promote to
   `register_stateful` and add the four `menu: None` primitives adjacent, mirroring
   `scrollbar_off/auto/on` + `cycle_scrollbar` verbatim (all sit far above `save_settings`, preserving the
   last-command invariant — spec §3.5):
```rust
// View menu — render mode: set-per-state primitives (palette-only) + stateful cycle representative.
r.register("view_live_preview",       "View: Live Preview",       None,
    |c| { c.editor.set_render_mode(crate::editor::RenderMode::LivePreview, c.clock.now_ms()); CommandResult::Handled });
r.register("view_review",             "View: Review",             None,
    |c| { c.editor.set_render_mode(crate::editor::RenderMode::Review, c.clock.now_ms()); CommandResult::Handled });
r.register("view_source_highlighted", "View: Source Highlighted", None,
    |c| { c.editor.set_render_mode(crate::editor::RenderMode::SourceHighlighted, c.clock.now_ms()); CommandResult::Handled });
r.register("view_source_plain",       "View: Source Plain",       None,
    |c| { c.editor.set_render_mode(crate::editor::RenderMode::SourcePlain, c.clock.now_ms()); CommandResult::Handled });
r.register_stateful("cycle_render_mode", "Render Mode", Some(MenuCategory::View),
    |e| MenuMark::Value(match e.active().view.mode {
        crate::editor::RenderMode::LivePreview       => "Live",
        crate::editor::RenderMode::Review            => "Review",
        crate::editor::RenderMode::SourceHighlighted => "SRC-HI",
        crate::editor::RenderMode::SourcePlain       => "Source" }),
    |c| run(c, Command::CycleRenderMode));
```
   Remove the old `r.register("cycle_render_mode", "Cycle Render Mode", Some(MenuCategory::View), |c| run(c, Command::CycleRenderMode));`
   line (replaced by the stateful registration above).
4. **`registry.rs`** — Review-only guards on the three diagnostic-action handlers (spec §2.5), each a
   leading short-circuit BEFORE the existing `valid_for` check:
   - `quick_fix`: `if !crate::diagnostics_run::should_show_diagnostics(c.editor) { c.editor.status = "no diagnostic here".into(); return CommandResult::Handled; }`
   - `diag_next` and `diag_prev`: `if !crate::diagnostics_run::should_show_diagnostics(c.editor) { return CommandResult::Handled; }`

### Existing-test migrations owned by T4 (the Review-only guards no-op the three diag commands outside Review)
Found via `rg -n 'quick_fix|diag_next|diag_prev' wordcartel/src -g '*.rs' | rg 'fn |dispatch_id|reduce\('`.
Each seeds diagnostics and drives one of the three commands in default LivePreview; the §2.5 guard now
short-circuits them. Fix: `e.active_mut().view.mode = crate::editor::RenderMode::Review;` at setup
(enabled is default-true). All six:
- **`app.rs::quick_fix_applies_suggestion_as_undoable_edit`** — Ctrl+. → overlay → Enter → apply. Set Review
  (else the overlay never opens and `assert!(e.diag.is_some())` fails).
- **`app.rs::quick_fix_on_stale_diagnostics_is_noop_no_overlay`** — **JUDGMENT CALL:** set Review so the
  mode guard passes and the *stale `valid_for`* guard is what refuses the overlay — otherwise the test would
  pass because the mode guard hid it (wrong reason), no longer isolating the stale-version guard it exists for.
- **`app.rs::diag_next_prev_move_caret_with_wrap`** — set Review (else `diag_next`/`diag_prev` no-op and the
  caret never moves).
- **`app.rs::quick_fix_refuses_stale_apply_after_concurrent_edit`** — set Review (the overlay-open step needs
  the mode gate to pass so the concurrent-edit/stale-apply refusal is what's exercised).
- **`registry.rs::diag_next_into_fold_auto_unfolds`** and **`registry.rs::diag_prev_into_fold_auto_unfolds`** —
  set Review before the `dispatch_id(&mut ed, "diag_next"/"diag_prev")` call.
- **Reviewed, NO change:** `app.rs::open_diag_clears_siblings_and_open_others_clear_diag` and
  `mouse.rs::{diag_click_applies_selected_row, click_outside_diag_closes}` call `Editor::open_diag` /
  `search_ui::diag_apply_selected` **directly** (simulating an already-open overlay), bypassing the gated
  `quick_fix`/`diag_*` commands — unaffected by the §2.5 guards. (These direct paths are intentionally NOT
  gated; see the mouse-affordance note in the Scope guard below.)

**Conformance check (run now):** `palette_is_exhaustive_over_the_registry` (the four primitives + cycle are
non-hidden rows → must appear), `every_persisted_setting_has_a_command` (unaffected — no new persisted key),
and the menu-subset invariant all stay green.

**Verify:** RED tests green; the six T4 migrations green; rotate + all registry/menu/palette tests green; clippy clean.

**Commit:** `E7 T4: render-mode command surface — set_render_mode setter, view_* + stateful cycle, Review-only diag actions`

---

## Task 5 — Keymap: bind `view_review` in both presets + WordStar `f1` cycle

**Goal:** pin the deferred `view_review` chord (conflict-checked against the real tries) and close the
WordStar `cycle_render_mode` gap (spec §3.4).

**Chord conflict-check (performed against the real `static CUA`/`static WORDSTAR` tables in `keymap.rs`):**

- **Candidate: `alt-r`** (mnemonic: **R**eview). Verification:
  - **CUA:** `grep -nE '"alt-r"' keymap.rs` → **no match**. CUA's `alt-` plane is `{u,l,c,t,shift-t,shift-l,j,space,shift-j,\,s,o,up,down,shift-up,z,shift-z,shift-x,b,shift-c,shift-v,comma,period,left,right}` — `r` is unused. All CUA `alt-` bindings are single chords (no `alt-r …` prefix), so no collision/shadow. FREE.
  - **WordStar:** the `WORDSTAR` table has **no `alt-` bindings at all** (only `ctrl-*`, the `ctrl-q`/`ctrl-k` prefix families, plain keys, and `f10`). `alt-r` cannot collide with or shadow a `ctrl-q`/`ctrl-k` prefix. FREE — passes `wordstar_has_no_chord_collisions_or_prefix_shadows`.
  - Same chord in both presets → the simplest law-7 hint story.
  - **Fallback if the human prefers an F-key** (consistent with the F1 cycle): `f7` is also free in both
    (CUA uses f1/f3/shift-f3/f8/shift-f8/f10; WordStar uses only f10). Recorded, not chosen — `alt-r` wins on mnemonic.
- **WordStar `f1`:** `grep -nE '"f1"' keymap.rs` shows `f1` only in the CUA table (+ a doc comment). FREE in WordStar.

### TDD — RED first
`keymap.rs` tests — use the REAL resolution API exactly as `wordstar_new_chords_resolve` does:
`parse_seq(s).unwrap()` → `KeyTrie::resolve(&seq) -> Resolution::Command(CommandId("…"))`, and build the
config with `crate::config::KeymapConfig { preset: "…".into(), patches: vec![] }`:
```rust
#[test]
fn view_review_binds_in_both_presets() {
    for preset in ["cua", "wordstar"] {
        let cfg = crate::config::KeymapConfig { preset: preset.into(), patches: vec![] };
        let (t, w) = build_keymap(&cfg, &Registry::builtins());
        assert!(w.is_empty(), "{preset}: no warnings: {w:?}");
        assert!(matches!(t.resolve(&parse_seq("alt-r").unwrap()),
            Resolution::Command(CommandId("view_review"))), "{preset}: alt-r → view_review");
    }
}
#[test]
fn wordstar_f1_cycles_render_mode() {
    let cfg = crate::config::KeymapConfig { preset: "wordstar".into(), patches: vec![] };
    let (t, _) = build_keymap(&cfg, &Registry::builtins());
    assert!(matches!(t.resolve(&parse_seq("f1").unwrap()),
        Resolution::Command(CommandId("cycle_render_mode"))));
}
```

### GREEN — implementation
1. **`static CUA`** — add under the `// View` group:
```rust
("alt-r", "view_review"),
```
2. **`static WORDSTAR`** — add (e.g., beside the `f10` escape-hatch / View additions):
```rust
("f1",    "cycle_render_mode"),
("alt-r", "view_review"),
```
   The other three `view_*` primitives stay palette-first (no chord) in both presets (spec §3.4).

**Verify:** RED tests green; `both_presets_resolve_against_builtins`, `hints_reresolve_on_preset_switch`,
`custom_bind_surfaces_in_menu_and_palette`, and `wordstar_has_no_chord_collisions_or_prefix_shadows` all green.

**Commit:** `E7 T5: bind view_review (alt-r) in both presets + WordStar f1→cycle_render_mode`

---

## Task 6 — Integration & guardrail tests across the seam and command stack

**Goal:** lock the cross-task behaviors that only exist once T3 (seam) + T4 (commands/overlays) are both in:
every interceptor edit family re-arms in Review, the mouse path fires once, and nothing arms outside Review.
(Spec §7.2.) These are added AFTER the behavior exists (integration characterization); each must be RED if
its guard were removed.

**Files:** `wordcartel/src/app.rs` (test module), reusing the `reduce` harness from T3.

Add:
1. **Quick-fix suggestion via the diag overlay** — open a `DiagOverlay` on a real spelling diagnostic in
   Review, drive `Enter` through `reduce` (→ `diag_overlay::intercept` → `diag_apply_selected` suggestion
   branch → `editor.apply`), assert `recheck_due_at == Some(now + debounce_ms)`. Repeat the same overlay
   edit with the buffer in LivePreview → `recheck_due_at == None`.
2. **Search-replace via the search overlay** — with the search overlay open in Review, drive the
   replace-all (or step-apply) path through `reduce` (→ `search_ui::intercept` → `search_replace_all` →
   `active_mut().apply`); assert armed in Review, not armed in LivePreview.
3. **Prompt-held job result** — with a prompt open in Review, feed `Msg::FilterDone` through `reduce`
   (→ `prompts::intercept` → `jobs_apply::apply_filter_done` → `Buffer::apply`); assert armed in Review,
   not armed in LivePreview.
4. **Mouse quick-fix single-fire** — in Review, with the diag overlay open on a suggestion, drive the mouse
   click-apply (normal-match path, `Msg::Input(Event::Mouse)` → `mouse::handle` → `diag_apply_selected`)
   through `reduce`; clear `recheck_due_at` immediately before, then assert it becomes `Some(now + debounce_ms)`
   from exactly the one seam call (guards against a re-introduced per-path re-arm double-arming — spec §7.2).
5. **Class-B non-edit re-arm** (`search_ui.rs` test if not already covered in T2): applying the
   **ignore** / **add-to-dictionary** overlay branches (which do NOT change `document.version`) still arms
   in Review and not outside — confirming the seam's version gate doesn't swallow the non-edit trigger.

**Verify:** all new tests green; full `cargo test -p wordcartel -p wordcartel-core` green.

**Commit:** `E7 T6: integration tests — interceptor-family arming, mouse single-fire, non-edit re-arm`

---

## Task 7 — Final gate sweep (no new behavior)

**Goal:** confirm every merge GATE before the branch is offered for the whole-branch review.

1. `cargo test -p wordcartel -p wordcartel-core` — green across lib + oracle suites.
2. `cargo build -p wordcartel -p wordcartel-core` and `cargo test --no-run` — warning-free for touched crates.
3. `cargo clippy --workspace --all-targets` — clean (workspace `all = "deny"`); no new `too_many_lines`
   (or an item-local `#[allow]` with a one-line reason on `reduce_dispatch` if it landed >100).
4. The five command-surface invariant GATEs green: `palette_is_exhaustive_over_the_registry`,
   `every_persisted_setting_has_a_command`, `hints_reresolve_on_preset_switch`,
   `both_presets_resolve_against_builtins`, `custom_bind_surfaces_in_menu_and_palette`.
5. `module_budgets::app_rs_stays_a_thin_dispatch_hub` green (the reduce split thins, not grows, the hub).
6. `scripts/smoke/run.sh` — run it; quote the one-line summary verbatim in the pre-merge report
   (mandatory-run / advisory-pass; a red or SKIP result is surfaced, not a blocker).

**No commit** unless step 3/5 required an item-local allow; if so:
`E7 T7: item-local too_many_lines allow on reduce_dispatch (flat interceptor dispatch)`.

---

## Scope guard (from the spec, restated so no task drifts)
- **Seq-1 only.** Embedded Harper backend unchanged — no `Cargo.toml` edits, no `harper-ls`/LSP work,
  `diag_cfg.linters` stays the unused placeholder (spec §5).
- **No struct changes to `DiagnosticsConfig`.** `enabled` stays master; `grammar` untouched.
- **Render mode is not persisted** — no `SettingsSnapshot`/config key added (law 2 unaffected).
- **`render.rs::gather_row_ctx` `plain_source`, `derive.rs::LayoutKey`, `nav.rs::layout_line_on_demand`,
  `input.rs`** — NO production edits (spec §1.2 sites 2/4/5/7); the only render.rs change is the T2
  `diag_active` display gate.
- **§2.5 gates ONLY the three keyboard commands** (`quick_fix`/`diag_next`/`diag_prev`). The mouse
  diagnostic paths (`mouse.rs` → `chrome_geom::diag_row_at` → `search_ui::diag_apply_selected`, and
  `Editor::open_diag`) are NOT gated and MUST NOT be touched — they are only reachable once the diag overlay
  is already open, and the overlay opens via `quick_fix` (now Review-only), so the affordance is consistent
  without a separate gate. Adding a mouse gate would be a design change beyond the approved §2.5 — out of scope.
