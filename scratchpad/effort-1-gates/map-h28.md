# H28 map — the two "sticky warning" picker tests and the pumping convention

Repo: `/home/jkeim/projects/groundwords`, branch `main`, clean tree at time of mapping.
Scope: `wordcartel/src/`. `.claude/worktrees/*` copies excluded (duplicate source trees, not
distinct code).

## 1. The two tests, verbatim

File: `wordcartel/src/prompts.rs`, module `prompts::tests` (`#[cfg(test)] mod tests` at
line 443, `use super::*; use crate::test_support::TestClock;`).

```rust
    /// A17 T5 (F4 Warning table, prompt-input refusals row): an empty Save-As path refusal
    /// is a Sticky Warning. Migrated (Task 21) from the retired `save_as_submit` to the
    /// picker path — an empty field yields `CommitOutcome::Nothing`, which
    /// `commit_destination` turns into the SAME message/kind/lifetime. Driven through the
    /// REAL intercept, not `commit_destination` directly (see the commit-arm's own
    /// end-to-end tests in `file_browser_commit.rs` for why).
    ///
    /// DELIBERATELY does NOT pump the async listing (unlike the audit applied elsewhere —
    /// see the parent-row-highlight task report). This is a SEPARATE, pre-existing property
    /// of Row 1, not the defect that audit fixed: Row 1 fires on ANY highlighted directory
    /// whenever the field is EMPTY, by design — `FileBrowser::highlight_navigated`'s gate is
    /// `navigated || trimmed.is_empty()`, and a bare Enter on an untouched highlight with
    /// nothing typed is treated as an ordinary browse gesture. Since `std::env::temp_dir()`
    /// is never filesystem root, its listing always carries a ".." row, so IF this test
    /// pumped that listing, Enter would descend into the parent directory instead of
    /// reaching `CommitOutcome::Nothing` — the "empty path" warning would never fire once a
    /// real listing has landed. Confirmed live (pump added, ran, status came back empty
    /// instead of "save-as: empty path"; reverted) — reported as a FINDING in the task
    /// report, not fixed here: whether Row 1 should ever cede to Row 2 on an untouched
    /// directory highlight with an empty field is a design question, not a mechanical one.
    #[test]
    fn save_as_empty_path_is_a_sticky_warning() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs = crate::test_support::test_fs();
        e.open_destination_picker(&fs, &tx, crate::file_browser::DestinationPurpose::SaveAs,
            std::env::temp_dir(), "   ".into());
        crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Enter);
        assert_eq!(e.status_text(), "save-as: empty path");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    /// A17 T5: an empty Write-Block path refusal is a Sticky Warning. Migrated (Task 21)
    /// from the retired `block_write_submit` — see the Save-As twin above, INCLUDING the
    /// same deliberate non-pump: confirmed to break identically if pumped (same finding).
    #[test]
    fn block_write_empty_path_is_a_sticky_warning() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 0, end: 1, hidden: false });
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs = crate::test_support::test_fs();
        e.open_destination_picker(&fs, &tx, crate::file_browser::DestinationPurpose::WriteBlock,
            std::env::temp_dir(), "   ".into());
        crate::test_support::press_key_fb(&mut e, &fs, &tx, crossterm::event::KeyCode::Enter);
        assert_eq!(e.status_text(), "write block: empty path");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }
```

`block_write_empty_path_is_a_sticky_warning` is the "Write-Block twin" referenced in the
backlog item; its real name is exactly that — not a paraphrase.

Lines: `save_as_empty_path_is_a_sticky_warning` 480-491; `block_write_empty_path_is_a_sticky_warning`
496-509 (doc comments start at 459 / 493 respectively).

## 2. What each asserts, and by what route

Both:
- Open a Destination-mode picker via `Editor::open_destination_picker` (the real production
  opener — `editor.rs:1013`), seeded at `std::env::temp_dir()` (never filesystem root) with
  field `"   "` (whitespace-only).
- Drive **one** `Enter` keypress through `crate::test_support::press_key_fb`, which builds a
  real `Registry`/`Keymap`/`DispatchCtx` and calls `crate::file_browser_intercept::intercept`
  — the REAL intercept, not a direct call to `commit_destination` or
  `classify_destination_enter`. This is the real production dispatch path for a picker
  keystroke (same one `app::reduce` delegates to).
- Assert `status_text()` is `"save-as: empty path"` / `"write block: empty path"`, kind
  `Warning`, lifetime `Sticky`.
- **Do NOT call `crate::test_support::pump_listing`** — no `Msg::ListingDone` is ever
  delivered before the `Enter`. This is documented as deliberate in both doc comments, with
  the Save-As one containing a full explanation and a "confirmed live, then reverted" note.

So: real intercept, not a handler call — but the async listing is left permanently pending.

## 3. The pumping mechanism

`wordcartel/src/test_support.rs`:

```rust
/// Deliver one pending `Msg::ListingDone` from the channel into the editor. The listing
/// runs on its own thread, so a test that drives Enter/open must pump the result to
/// observe the outcome. Bounded wait — never hangs a test run.
pub(crate) fn pump_listing(e: &mut crate::editor::Editor,
    rx: &std::sync::mpsc::Receiver<crate::app::Msg>) -> bool
{
    match rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(crate::app::Msg::ListingDone { epoch, dir, result }) => {
            crate::file_browser::apply_listing_done(e, epoch, dir, result);
            true
        }
        _ => false,
    }
}

/// Open the file browser via the real async path (`Editor::open_file_browser`) and pump
/// its initial listing so `fb.entries` is populated before the caller's first assertion.
pub(crate) fn open_and_pump(e: &mut crate::editor::Editor, dir: std::path::PathBuf)
    -> std::sync::mpsc::Receiver<crate::app::Msg>
```

`apply_listing_done` (`wordcartel/src/file_browser.rs:435`) is the merge function itself
(epoch-gated, discards stale/inert listings); `pump_listing` is the test-only helper that
receives one `Msg::ListingDone` off the channel and feeds it to `apply_listing_done`,
exactly as the production run loop would. `open_and_pump` is sugar for "open Select-mode +
pump once" — Destination-mode pickers pump the same way but call `pump_listing` directly
after `open_destination_picker`, since `open_and_pump` only wraps `open_file_browser`.

There is also a recents-picker analogue with the same discipline but a different message
(`Msg::RecentsProbed` / `apply_recents_probed`), used by `mouse.rs`'s
`open_recents_and_pump` helper and directly by `recents.rs`/`app.rs`.

**Canonical example of correct pumping** (best single illustration, explicitly documented as
such in its own doc comment): `wordcartel/src/e2e.rs`,
`fn journey_open_save_export_saveas_reopen()` (line 3048), via the `Harness::pump_listing`
wrapper (line 229) and `Harness::key` (line 216, → `step` → the real `app::reduce`/`advance`
loop). Its module comment states: "The listing is pumped before every assertion on
`fb.entries`, because it arrives from another thread (§6.3) — an unpumped picker is empty, a
state real usage never reaches."

The most on-point canonical example for THIS exact defect class, though, is
`file_browser_commit.rs::typing_a_name_after_the_listing_lands_commits_not_descends` (line
1000) — the regression test for the "parent-row-highlight" defect this whole convention
exists to guard against (see §5).

## 4. The convention census

Every test in `wordcartel/src/` that opens a `FileBrowser` (Select, Destination, or Recents
mode) and, in most cases, drives at least one keystroke/click into it. `.claude/worktrees/*`
excluded as duplicate trees. Pure-function tests that call `classify_destination_enter`,
`resolve_field`, `apply_extension_policy`, `classify_enter`, `classify_highlight_target`,
`footer_target`, or `file_browser_row_at`/geometry helpers directly with no `Editor`/picker
in play are **excluded** from this census (they don't drive a picker at all — see the
counts at the end of each file's section for how many were excluded there).

Legend: **Pump** = was `pump_listing`/`open_and_pump`/the recents-probe equivalent called
before the input under test landed. **Route** = how the input was delivered: **real**
(through `file_browser_intercept::intercept`, `prompts::intercept`, `app::reduce`, or
`mouse::handle`/`mouse_file_browser`, incl. via the `press_*_fb` sugar which wraps the real
intercept) vs **direct** (a handler function called by the test, bypassing dispatch) vs
**none** (no input driven at all — picker opened and inspected, or a non-input event like
`Moved`/a generic key unrelated to commit).

### `file_browser.rs` (6 driving; 15 excluded as pure-function/no-picker)
| test | pump | route |
|---|---|---|
| `enter_on_unreadable_dir_stays_put_and_sets_status` | yes (before + after) | real (`app::reduce`) |
| `open_file_browser_enforces_xor` | n/a | none (opens only) |
| `a_query_keystroke_performs_no_directory_read` | n/a — `entries` seeded via the test-only **synchronous** `seed_listing` helper, never the async path | real (`press_char_fb`) |
| `stale_listing_after_close_and_reopen_is_discarded` | yes (`open_and_pump` ×2) + a manual `apply_listing_done` call for the stale epoch | none (tests listing-merge directly, no key) |
| `a_failed_descend_leaves_the_writer_exactly_where_they_were` | yes (before + after) | real (`press_enter_fb`) |
| `picker_enter_opens_through_the_injected_fs_not_a_hardcoded_realfs` | yes (asserted `true`) | real (`press_enter_fb`) |

### `file_browser_listing.rs` (2 driving)
| test | pump | route |
|---|---|---|
| `typing_in_destination_mode_narrows_the_listing_to_matching_files` | yes | real (`press_char`, local wrapper around the intercept) |
| `destination_mode_reveals_output_siblings_the_select_mode_hides` | yes (both the hand-built Destination half and the `open_and_pump` Select half) | none (no key; asserts derived state after listing) |

### `file_browser_commit.rs` (20 driving; 10 excluded as pure-function)
All 20 use `Editor` + a real `FileBrowser`. 18 of 20 open via `open_destination_picker` (or
the production `run_export_with_probe`), pump, then commit through the real intercept:
`row2_onto_an_extensionless_file_targets_that_exact_file`,
`row2_onto_markdown_or_plain_text_targets_that_exact_file`,
`row2_onto_a_foreign_format_pandoc_writes_offers_export_instead`,
`row2_onto_a_plain_text_foreign_format_is_still_refused`,
`a_refused_row2_creates_no_file_at_all`, `save_as_commits_end_to_end_from_enter`,
`typing_a_name_after_the_listing_lands_commits_not_descends`,
`arrowing_to_a_real_directory_still_descends_and_keeps_the_typed_field`,
`a_noop_nav_key_does_not_arm_row1_so_typing_then_enter_still_commits`,
`navigating_onto_an_entry_that_then_filters_out_does_not_leave_a_stale_flag_on_whatever_slides_in`,
`cancelling_the_overwrite_modal_clears_both_paired_fields`,
`a_half_cleared_overwrite_pair_refuses_loudly_instead_of_doing_nothing`,
`redirect_clears_the_pending_quit_drain_state`,
`confirming_the_overwrite_modal_writes_to_the_resolved_symlink_target`,
`a_trailing_separator_destination_is_refused_end_to_end_writes_nothing`,
`write_block_commits_end_to_end_from_enter`, `export_commits_end_to_end_from_enter_through`,
`export_enter_through_reproduces_run_export_with_probe_derivation` — **pump: yes, route: real**
for every one of these 18. Two doc comments (`redirect_clears_the_pending_quit_drain_state`,
`a_trailing_separator_destination_is_refused_end_to_end_writes_nothing`) explicitly say they
were **previously unpumped and were fixed** by the parent-row-highlight task, once the Row-1
guard in §5 made pumping safe for a non-empty field.

The remaining 2 build `FileBrowser` by hand with `entries` already populated (no async
listing ever spawned, so "pump" doesn't apply): `tab_copies_a_name_into_the_field_and_does_not_commit`
(route: real, `press_key_fb` Tab) and `a_click_on_a_file_in_destination_mode_copies_the_name_and_does_not_commit`
(route: real, `mouse::mouse_file_browser` via `DispatchCtx`). Neither reaches `commit_destination`.

### `prompts.rs` (2 driving — the H28 subjects)
| test | pump | route |
|---|---|---|
| `save_as_empty_path_is_a_sticky_warning` | **no — deliberately** | real (`press_key_fb` Enter → `file_browser_intercept::intercept`) |
| `block_write_empty_path_is_a_sticky_warning` | **no — deliberately** | real (same) |

### `mouse.rs` (5 driving)
| test | pump | route |
|---|---|---|
| `dwell_never_arms_during_drag_or_overlay` | n/a (hand-built empty `FileBrowser`, arming-check only) | none |
| `fb_wheel_scroll_moves_selection` | yes (`open_and_pump`) | real (`mouse::handle`) |
| `click_dir_enters` | yes (before + after the click's own re-descend) | real (`mouse::handle`) |
| `click_on_an_available_recents_row_opens_it_like_enter` | yes (recents-probe equivalent, via local `open_recents_and_pump`) | real (`mouse::handle`) |
| `click_on_an_unavailable_recents_row_refuses_like_enter` | yes (same) | real (`mouse::handle`) |

### `overlays.rs` (2 driving, not commit-related)
| test | pump | route |
|---|---|---|
| `every_overlay_is_active_xor_and_consumes_key_and_click` | no | real (`app::reduce` for a generic `'z'` key; a direct mouse-slot call for a stray click) — file_browser is one of 11 overlays exercised generically, Enter is never pressed |
| `every_overlay_consumes_moved_without_panic_or_data_loss` | no | real (direct mouse-slot call, `Moved` only) |

### `render_overlays.rs` (2 driving)
| test | pump | route |
|---|---|---|
| `the_destination_field_and_its_caret_are_painted_in_the_query_row` | yes | real (`press_char_fb`, typing only — no Enter) |
| `the_withholding_disclosure_is_painted` | yes (`open_and_pump`) | none (no key; render-only) |

### `render.rs` (2 driving)
| test | pump | route |
|---|---|---|
| `file_browser_windowed_slice_and_indicator` | yes (`open_and_pump` ×2) | none (`selected`/`scroll_top` hand-set, no key) |
| `file_browser_query_shows_caret_end_of_string` | **no** | none — `fb.query` set by direct field mutation, never through the intercept at all |

### `app.rs` (2 driving)
| test | pump | route |
|---|---|---|
| `file_browser_pgdn_home_end_and_enter_dispatches_visible` | yes (before nav, and after the Enter-descend) | real (`app::reduce`) |
| `file_browser_scrolled_descend_resets_window` | yes | real (`app::reduce`) |

### `save.rs` (1 driving)
| test | pump | route |
|---|---|---|
| `esc_out_of_a_drain_destination_picker_aborts_the_drain` | **no** | real (`press_key_fb` Esc) — Esc never reaches `classify_destination_enter`/`commit_destination`, so the missing pump is immaterial to the H28 guard, but it IS an un-pumped real-intercept keystroke against an open Destination picker |

### `session_restore.rs` (1 driving — AMBIGUOUS)
| test | pump | route |
|---|---|---|
| `file_browser_enter_on_file_opens_it_when_clean` | no | **ambiguous/direct** — opens via `open_file_browser`, then calls `open_into_current(&mut e, &crate::fsx::RealFs, &dir.join("note.md"))` **directly**. Its own inline comment says "simulate Enter via the browser's open path", but no `Enter` key, no `press_*_fb`, and no `intercept` call appears anywhere in the test — it never touches the picker's selection/highlight at all. This is a direct handler call mislabeled as an Enter simulation. |

### `recents.rs` (3 driving)
| test | pump | route |
|---|---|---|
| `an_unavailable_recent_is_refused_as_gone_not_as_indeterminate` | yes (recents-probe equivalent) | real (`press_enter_fb`) |
| `typing_narrows_the_recents_list_rather_than_clearing_it` | n/a — `open_recents` populates rows synchronously, no async fetch to pump | real (`press_char_fb`) |
| `backspacing_the_query_widens_the_recents_list_back_out` | n/a (same) | real (`press_char_fb` + `press_key_fb`) |

### `export.rs` (3 — open picker, drive no key)
`export_opens_a_destination_picker_pre_seeded_with_the_derived_path`,
`export_destination_picker_opens_without_pandoc_installed`,
`export_still_refuses_before_opening_any_picker` — none pump, none drive a key; each only
inspects the picker's seeded state immediately after opening (or asserts no picker opens).

### `blocks_marked.rs` / `chrome_geom.rs`
No tests drive the picker. `blocks_marked.rs:140` and `export.rs:303` and `prompts.rs:86` are
production code (`block_write`, `redirect_to_export`, the write-block prompt opener), not
tests. `chrome_geom.rs` has one `FileBrowser`-Destination-mode test
(`hit_testing_and_the_painter_agree_on_the_last_row_in_destination_mode`) but it is pure
geometry — no `Editor`, no input — excluded.

### Totals
- **51** test functions across 13 files open a `FileBrowser` (Select/Destination/Recents).
- Of those, **~46** actually drive at least one keystroke, click, or equivalent input event
  (the rest only inspect post-open state).
- Restricting to the class the H28 guard concerns — **an `Enter` driven at a Destination-mode
  picker to reach `classify_destination_enter`'s decision table** — there are exactly **20**
  such tests (18 in `file_browser_commit.rs` + the 2 in `prompts.rs`). **18/20 pump before
  the `Enter`; 2/20 do not — and those 2 are exactly `save_as_empty_path_is_a_sticky_warning`
  and `block_write_empty_path_is_a_sticky_warning`.** All 20 go through the real intercept —
  none of the 20 calls `commit_destination` or `classify_destination_enter` directly.
- Across the wider census (~46 driving tests), **only one** bypasses the real
  intercept/dispatch for a direct handler call: `session_restore.rs`'s
  `file_browser_enter_on_file_opens_it_when_clean` (§4, flagged ambiguous — its own comment
  claims to simulate Enter but does not).
- A further **5** tests drive real input at an un-pumped picker but the input is immaterial
  to the commit guard (Esc, a generic non-picker key, `Moved`, or a direct-field mutation
  that never touches the intercept): `esc_out_of_a_drain_destination_picker_aborts_the_drain`,
  `every_overlay_is_active_xor_and_consumes_key_and_click`,
  `every_overlay_consumes_moved_without_panic_or_data_loss`,
  `file_browser_query_shows_caret_end_of_string`, and (as covered above)
  `file_browser_enter_on_file_opens_it_when_clean`.

**Reading of the lesson's uptake**: outside the two tests H28 names, the "pump before
driving Enter at a Destination picker" convention is applied with no exceptions found —
including two tests (`redirect_clears_the_pending_quit_drain_state`,
`a_trailing_separator_destination_is_refused_end_to_end_writes_nothing`) whose own doc
comments record that they used to skip the pump and were fixed. The two H28 tests are not an
inconsistent application of the lesson; they are a **documented, deliberate, reasoned
exception** to it, recorded in-line with a "confirmed live, then reverted" note.

## 5. Is "the warning is unreachable once a listing lands" verifiable from the code?

**Yes — traceable end-to-end from the code as written, without needing to run anything.**

`commit_destination` (`file_browser_commit.rs:299`) computes the highlight from the picker's
*live* entry list, not from any cached/expected state:

```rust
let highlighted = fb.entries.get(fb.selected).cloned();
let highlight_navigated = fb.highlight_is_navigated();
```

`open_destination_picker` (`editor.rs:1013`) constructs the `FileBrowser` with
`entries: Vec::new()` and `selected: 0`, and only *starts* the listing
(`file_browser::start_listing`) — it never populates `entries` synchronously. So immediately
after `open_destination_picker` returns, `fb.entries.get(0)` is `None` ⇒ `highlighted = None`.

`classify_destination_enter` (`file_browser_commit.rs:77`), with `highlighted = None` and a
whitespace-only field (`"   ".trim() == ""`):

```rust
// Row 1 — a highlighted directory descends, ...
if let Some(e) = highlighted {                                   // None ⇒ skipped entirely
    if matches!(e.kind, EntryKind::Dir) && (highlight_navigated || trimmed.is_empty()) {
        ...
    }
}
// Row 2 — an empty field commits onto the highlighted FILE. ...
if trimmed.is_empty() {
    return match highlighted {
        Some(e) if matches!(e.kind, EntryKind::File) => { ... }
        _ => CommitOutcome::Nothing,                              // reached: None doesn't match
    };
}
```

With no listing pumped, Row 1 is skipped (`highlighted` is `None`) and Row 2 falls to the
`_ => CommitOutcome::Nothing` arm, which `commit_destination`'s `CommitOutcome::Nothing` arm
turns into the Sticky Warning both tests assert on. This is exactly why the two tests, as
written today, pass.

Once the listing lands (`apply_listing_done`, `file_browser.rs:435`, called by
`pump_listing`), `fb.entries` is populated by `file_browser_listing::rederive`, which sorts
directories before files and always synthesizes a `".."` row for a non-root directory —
`std::env::temp_dir()` is never filesystem root, so `entries[0]` becomes the `".."` `Dir`
entry. `fb.selected` is untouched by the merge (`apply_listing_done` only resets
`selected`/`scroll_top`/`navigated_name` on an actual directory *move*, `moved = p !=
fb.dir`, which is `false` here since `fb.dir` was already set to the target directory at
construction) — so `fb.selected` stays `0`, and `highlighted` becomes `Some(".." Dir entry)`.
Row 1's guard is now `matches!(Dir) && (false || true)` = **true** (the field is still
whitespace-only, so `trimmed.is_empty()` is the `true` half of the OR regardless of
`highlight_navigated`), so `classify_destination_enter` returns
`CommitOutcome::Descend(dir.parent())` instead of falling through to `CommitOutcome::Nothing`
— the Sticky Warning branch becomes unreachable via this path once a listing has landed.

This composition (`open_destination_picker`'s empty-`entries` start state × `commit_destination`'s
live-`entries` read × Row 1's `trimmed.is_empty()` OR-branch × `rederive`'s dirs-first-with-`..`
ordering × `apply_listing_done`'s no-op-on-same-dir reset) is exactly what the two tests' own
doc comments describe, and it is independently re-derivable from the code with no test run
required. The two tests' authors additionally recorded that they ran this live ("pump added,
ran, status came back empty instead of 'save-as: empty path'; reverted"), which is consistent
with but not required to establish the trace above.

## 6. Fixtures/helpers that silently skip pumping

None found that are silent about it. Every helper that opens a picker asynchronously and
skips the pump documents the skip inline at each call site (the two prompts.rs tests) or pumps
unconditionally as part of the helper itself:
- `test_support::open_and_pump` — always pumps (name says so).
- `test_support::pump_listing` — the pump primitive itself; callers choose whether to invoke it.
- `mouse.rs::open_recents_and_pump` (local) — always pumps (the recents-probe equivalent).
- `e2e.rs::Harness::pump_listing` / `pump_recents` — primitives, not auto-invoked by `open_*`.
- `file_browser_commit.rs::row2_enter_onto` (local helper used by 4 tests) — always pumps
  internally before returning, so no caller of it can forget.

No helper opens a Destination picker AND drives it into `commit_destination` territory
without either pumping internally or leaving the non-pump visible and commented at the call
site the way the two H28 tests do. The one genuinely quiet case is
`session_restore.rs::file_browser_enter_on_file_opens_it_when_clean` (§4): it opens a picker,
never pumps, and never drives any input into it at all (bypassing the intercept via a direct
`open_into_current` call) — so it cannot inherit or propagate the H28 pumping question to any
other test, but its misleading "simulate Enter" comment is worth noting for anyone auditing
this convention by grepping comments rather than reading bodies.
