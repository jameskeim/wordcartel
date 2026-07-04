# H1 — app.rs decomposition (mechanical module split)

**Status:** CLEAN — Codex spec review ×3 (r3 CLEAN) + Fable5 ×3 (r3 READY), 2026-07-04.
Plan-writer notes from the final Fable pass: (1) the polish-commit ENUMERATED LIST is the
operative checklist — the prose criterion also matches location-free mentions in the
residual app.rs/staying tests that rightly need no edit; (2)
`quit_after_save_cancelled_when_edited_during_flight` (app.rs:3307) is not in the family
list and calls only `apply_result` — it moves to jobs_apply.rs under the rule, and either
disposition compiles; the per-body pass decides.
**Effort:** H1 (backlog Theme H; `settled-direction`, sized Medium)
**Date:** 2026-07-04 · **Facts as of:** `e2c7667` (post-A6 merge; app.rs = 5,740 lines,
~2,000 production + 3,185 test)

## Why

`wordcartel/src/app.rs` is six distinguishable modules wearing one name. Every 2026-07
effort (export, themes, menu modes, A6) had to edit it — the standing merge-conflict
hotspot and the hardest file for reviewers and implementer subagents to hold. Effort P's
plugin event-hook dispatch lands in exactly `reduce`/registry territory; P's diff should
land in a file whose main content IS `reduce`, not at line 1,114 of a six-topic file.

This effort is a **mechanical, behavior-preserving split**. It adds no features, fixes no
bugs, and changes no behavior. Its value is review-ability, conflict isolation, and a
clean landing zone for Effort P.

## Decisions (user-approved 2026-07-04)

1. **`Msg` stays in app.rs** (fork 1 = A). The residual app.rs is the Elm core — `Msg` +
   `reduce` + the run loop. The nine files importing `crate::app::Msg` (clipboard,
   diagnostics_run, export, filter, transform, registry, test_support, file_browser,
   e2e) are untouched. `jobs_apply.rs` imports `crate::app::Msg` like every other
   satellite already does. No `msg.rs`, no re-export aliases.
2. **The A6 overlay glue stays in app.rs** (fork 2 = A): `keep_overlay_visible`,
   `hydrate_overlays`, `dispatch_overlay_command`, `preview_selected_theme`,
   `outline_jump_to`, and test-only `menu_select_for_test`. They are `reduce`'s limbs
   (~80 production lines); the fifteen mouse.rs/render.rs call sites stay untouched.
   Backlog note (recorded at ship time): if the overlay-mouse-parity follow-up grows this
   glue substantially, THAT effort creates `overlay_ui.rs`.

## Module map

Four new files in `wordcartel/src/`, each declared in **lib.rs** (module declarations
live there, not main.rs). Line anchors are current app.rs positions at `e2c7667`.

### `jobs_apply.rs` — job/message application (~300 prod lines)

| fn | today | moves as |
|---|---|---|
| `apply_result` | `pub` :118 | `pub` |
| `apply_job_result` | `pub` :180 | `pub` |
| `apply_outcome` | `pub` :190 | `pub` |
| `apply_panic` | private :198 | private (called only by `apply_outcome`) |
| `apply_job_outcome` | `pub` :236 | `pub` |
| `drive_quit_drain` | `pub` :248 | `pub` |
| `apply_filter_done` | private :287 | `pub(crate)` (called by `reduce`) |
| `apply_transform_done` | private :333 | `pub(crate)` (called by `reduce`) |
| `apply_export_done` | private :351 | `pub(crate)` (called by `reduce`) |
| `insert_paste_text` | private :769 | `pub(crate)` (called by `apply_clipboard_paste` AND `reduce`'s `Msg::Input(Event::Paste)` arm :1934) |
| `apply_clipboard_paste` | private :793 | `pub(crate)` (called by `reduce`) |
| `apply_clipboard_availability` | private :808 | `pub(crate)` (called by `reduce`) |

The clipboard trio (:769-:813) sits between seams today; it handles
`Msg::ClipboardPaste`/`Msg::ClipboardAvailability` and belongs here.

### `session_restore.rs` — session/resume restoration (~90 prod lines)

`apply_resume` (:407), `load_marks_from_entry` (:421), `load_block_from_entry` (:441),
`restore_resume` (:455), `restore_scratch` (:481), `open_into_current` (:505) — all
`pub` today, all move as `pub`. `open_into_current → restore_resume` is
module-internal; `run()` (staying in app.rs) and workspace.rs call in.

### `prompts.rs` — prompt submits & file dialogs (~210 prod lines)

| fn | today | moves as |
|---|---|---|
| `open_save_as` | `pub` :532 | `pub` |
| `expand_path` | `pub` :541 | `pub` |
| `save_as_submit` | `pub` :551 | `pub` |
| `block_write_submit` | `pub` :575 | `pub` |
| `perform_block_write` | private :588 | private (callers both in-module) |
| `perform_save_as` | private :596 | private (callers both in-module) |
| `request_new` | `pub` :609 | `pub` |
| `resolve_prompt` | `pub` :618 | `pub` |
| `submit_filter_line` | private :728 | `pub(crate)` (called by `reduce`'s minibuffer Enter arm :1679) |
| `goto_line_submit` | `pub(crate)` :751 | `pub(crate)` |

`resolve_prompt` calls `crate::jobs_apply::drive_quit_drain` (:641, :653) — the one
cross-module call among the four extractions (prompts → jobs_apply, one direction, not
circular).

### `search_ui.rs` — search-and-replace + quick-fix UI (~180 prod lines)

All ten items are private today and called only from `reduce`; each fn becomes
`pub(crate)`: `search_sync` (:887), `search_step` (:898), `search_cancel` (:908),
`search_replace_all` (:918), `search_step_apply` (:952), `search_step_skip` (:978),
`search_step_rest` (:984), `search_pin` (:1004 — stays **private**, called only by
`search_step_apply`/`search_step_skip`), `diag_apply_selected` (:1014). The
`SearchReplacePlan` type alias (:916) moves and stays private. `outline_jump_to` does
NOT move here — it is overlay glue (decision 2).

### Residual `app.rs` (~1,800 prod lines, one topic: the event core)

`Msg` (:26) + `impl Debug for Msg` (:75) + `ExitReason` (:70); the A6 overlay glue
(two disjoint ranges: :819-:885 and :1094-:1111 — the :887-:1092 span between them is
the search_ui seam, which moves); `reduce` (:1114, ~900 lines); `step`/`SystemClock`/`advance`/`run`/
`recompute_scrollbar_visible`/`recompute_menu_bar`/`reconcile_mouse_capture`/
`persist_session`(+`_for_test`) (:2027-:2553); the reducer-level test module (below).

## Visibility contract (the "mechanical" definition)

- Moves are **verbatim**: function bodies, doc comments, and attributes move unchanged.
  The ONLY permitted line-level changes are (a) `use` statements in each file's header,
  (b) visibility keywords listed in the tables above, (c) cross-module call paths
  (`drive_quit_drain(...)` → `crate::jobs_apply::drive_quit_drain(...)` etc.), and
  (d) module doc-comments (`//!`) at each new file's top. **Any other changed line is a
  defect** — the per-commit diff must read as pure motion.
- Private → `pub(crate)` escalations happen ONLY where the tables above say so (six in
  jobs_apply, one in prompts — `submit_filter_line`, eight of ten in search_ui).
- **No tightening.** Existing `pub` items stay `pub` even where `pub(crate)` would
  compile (visibility minimization is a separate, never-scheduled cleanup — mixing it in
  would destroy the pure-motion review property). `run`/`ExitReason` must stay `pub`
  regardless (main.rs is the bin crate: `wordcartel::app::run`, `app::ExitReason`).
- **No re-exports.** External call sites update to the new paths honestly:
  - `crate::app::apply_outcome` → `crate::jobs_apply::apply_outcome` — 14 sites:
    save.rs:329/:348/:369/:393/:424/:433/:487/:770, reconcile.rs:120/:151/:176,
    swap.rs:567/:583, file.rs:315.
  - `crate::app::open_save_as` → `crate::prompts::open_save_as` — registry.rs:159,
    save.rs:144.
  - `crate::app::request_new` → `crate::prompts::request_new` — registry.rs:146.
  - `crate::app::open_into_current` → `crate::session_restore::open_into_current` —
    workspace.rs:75; `crate::app::restore_resume` →
    `crate::session_restore::restore_resume` — workspace.rs:85.
  - mouse.rs, render.rs, e2e.rs, main.rs: **zero changes** (everything they reference
    stays in app.rs).

## Test migration

**The rule:** a test moves with the module whose functions it calls **directly**; a test
that reaches the moved code only through `reduce(...)` stays in app.rs — it is a reducer
test regardless of which seam it exercises. **The spanner exception (Codex r1 + Fable
I1/N1, two-pronged): (a) the QUIT-DRAIN FAMILY stays in app.rs IN ITS ENTIRETY —
enumerated below, including its members that direct-call only prompts fns; (b) outside
that family, any test that directly calls functions of TWO OR MORE extracted modules
stays in app.rs as a flow-integration test. Stayers get path rewrites as their only
edit.** The quit-drain family's direct calls:
`resolve_prompt` (app.rs:3087/:3114/:3117/:3139/:3174/:3196), `apply_job_outcome`
(:3093/:3175), and `save_as_submit` (:3201; Codex-enumerated — the plan's per-test
pass remains the authoritative rewrite checklist) — plus the two Fable-found spanners
outside that family: `save_and_quit_sets_pending_after_save_and_exits_on_matching_result`
(app.rs:3207 — calls `resolve_prompt` :3218 AND `apply_outcome` :3221) and
`save_as_writes_new_path_and_rekeys` (app.rs:5081 — calls `save_as_submit` :5092 AND
`apply_outcome` :5093). All stay in app.rs with their direct calls rewritten to the new
paths (`crate::prompts::resolve_prompt`, `crate::jobs_apply::apply_job_outcome`, …).
The implementation plan classifies each of the 139 tests explicitly BY READING ITS BODY,
not by name-family — Codex r1 confirmed families are mixed: `replace_all_*` and
`invalid_regex_replace_all_*` reach search code only through `reduce` (:4513, :4725 —
they STAY), while the goto family splits (`goto_line_jumps_…` :3545 goes through
`reduce` and stays; the tests at :3566/:3578 call `goto_line_submit` directly and MOVE).
No numeric quota — the rule plus the per-test pass decides; stayed tests that name moved
fns get path rewrites as their only edit.

**No helper promotion (user decision 2026-07-04, on Fable I2).** The shared test
helpers — `cua_keymap()` (:2565), `quit_tmp()` (:3050), `f10()` (:2598), `build_km()`
(:5289) — and the `static SEQ: AtomicU32` (:2563; six use sites — five tests at :2997/
:3614/:3665/:3712/:3755 plus the `quit_tmp` body :3053) all STAY in `app.rs::tests`: Fable I2 intersected every helper-use line
(92 of them) against the tests that move under the rule above — zero overlap; every
consumer is a reduce-driven, quit-drain, or spanner test that stays. Moving tests need
only `TestClock`/std (already in test_support.rs). If a future effort moves a test that
needs one of these helpers, that effort promotes exactly what it needs.

No new tests are required (a mechanical refactor is verified by the existing 975), and
no test may be weakened, deleted, or have assertions altered — renamed imports
(`use crate::jobs_apply::…`) are the only permitted test edits beyond the physical move.

## Process & gates

- **One commit per module extraction**, in dependency order: (1) `jobs_apply.rs`,
  (2) `session_restore.rs`, (3) `prompts.rs` (its `resolve_prompt` needs jobs_apply
  already in place), (4) `search_ui.rs`, (5) a final polish commit: the
  doc-comment references to moved fns gain the new module names — editor.rs:26
  (`drive_quit_drain`)/:34/:363/:418/:429, workspace.rs:206/:216 (`open_into_current`),
  transform.rs:99 (`resolve_prompt`) and :139 ("`apply_transform_done` in app.rs"),
  editor.rs:430 (`apply_job_result`), editor.rs:642 (`diag_apply_selected`),
  reconcile.rs:130/:132 (`apply_panic`), save.rs:61 (`perform_save_as`), and the
  residual app.rs:23 section header ("Msg, apply_result, reduce" — goes stale)
  (Codex r1 + Fable M2/N3; criterion: any comment naming a moved fn or its app.rs home), lib.rs `mod` ordering matches its existing grouping,
  and the backlog/ledger bookkeeping. Blame recovery is `git blame -C -C` (a split is a
  content copy, not a rename; `--follow` alone won't track it — the backlog line
  claiming otherwise is corrected at ship time).
- **Every commit** passes the full gates: `cargo test -p wordcartel-core -p wordcartel`
  green, `cargo clippy --workspace --all-targets` clean (deny gate), warning-free build.
  NO `cargo fmt`; moved code keeps its hand formatting byte-for-byte.
- Pre-merge: the standard two final gates (opus whole-branch + Codex GO/NO-GO), plus
  `scripts/smoke/run.sh` quoted verbatim (advisory). The whole-branch reviewer's special
  charge here: verify the moves are verbatim — checked PER COMMIT against its parent,
  modulo the clause (a)-(d) permitted changes (Fable M3: `resolve_prompt` legitimately
  differs from branch base at :641/:653 — edited under clause (c) in commit 1, moved in
  commit 3) — and that no logic line changed anywhere in the branch.

## Non-goals (explicit)

- No visibility tightening (`pub` → `pub(crate)`) anywhere.
- No `msg.rs`; no fifth `overlay_ui.rs` module (revisit inside the overlay-mouse-parity
  follow-up if its glue grows).
- No render.rs split (rides E3 per the backlog), no block_tree.rs changes (deliberate
  non-split — fuzz-hardened, blame-stable), no logic/behavior/formatting changes of any
  kind.

## Ship-time bookkeeping

Backlog: H1 → SHIPPED with the residual-file line counts; correct the H1 entry's
`git log --follow` claim to `git blame -C -C`. Memory: working order advances (next =
B1+B2 or per user). Ledger: standard per-task lines.
