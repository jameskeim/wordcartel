# Fable verification of I1 — mouse click-away on a quit-owned picker

Reviewer: Claude Fable 5.1 (`claude-fable-5-1`), 2026-09-11.
Snapshot: `fix/quit-save-durability`, uncommitted against `d3aea92`, verified against
`docs/reviews/evidence/2026-09-11-premerge-cleanup/source-sha256.json` (26/26 match before
and after the review; `lib.rs` carried a temporary probe registration in between and was
restored to its manifest hash).

Inputs: `REVIEW_BRIEF.md`, `followup.diff`, `previous-fable-review.md`, `mouse-red.log`,
`mouse-green.log`, and the live source for `mouse.rs` (`mouse_file_browser`, `handle`,
`route_overlay`), `file_browser.rs` (`close_overlay`, `cancel_destination`), `quit.rs`
(`cancel`, `in_progress`, `allow_*`), `file_browser_intercept.rs` (Esc arm), `prompts.rs`
(`open_save_as_picker`), `chrome_geom.rs` (overlay geometry), and the maintained regression in
`durability_regressions.rs`. Prior approval was treated as context, not proof.

## Verdicts

- **I1: RESOLVED.** The click-away arm of `mouse_file_browser` now calls
  `file_browser::close_overlay`, the same helper the overlay registry row uses. For a picker
  with `quit_save_owner` set, that delegates to `cancel_destination`, which clears the
  picker, `pending_save_as`, the overwrite pair, `pending_write_block`, `pending_export`, and
  calls `quit::cancel`. The end state is identical to the keyboard Esc route (probe p4
  compares the full tuple, status text included).
- **Design compliance: PASS.** This is the brief's option (a): minimal, one production
  line, routed through the shared helper. Non-owned pickers (manual Save As, close-buffer,
  export) still take the plain `file_browser = None` path, so M3 is retained by design and
  not silently broadened (probe p1). Only the outside-the-rect branch changed; clicks on the
  dialog's borders, title, field, and footer leave the quit-owned request intact (probe p2).
  Nothing else in the production delta differs from the previously reviewed snapshot.
- **Code quality: PASS.** The change matches house style and the neighbouring registry row,
  introduces no new state, borrow, or unwrap, and the regression test drives the production
  path (`app::reduce` with a real `Event::Mouse`) rather than calling the slot directly.
  `mouse-red.log` fails at the `pending_save_as` assertion only after the picker-closed
  assertion passed, which proves the red run exercised the click-away arm and not some
  unrelated failure. Clippy, build, and test-build are warning-free.
- **Recommendation: GO for the current snapshot.** Critical: 0. Important: 0. Minor: 2 (both
  new, neither introduced by the delta). Retained observations unchanged.

## Findings

### N1 (Minor, pre-existing) — click-away rect is sized from `entries.len()`, the drawn box from the row ledger

- **Where:** `mouse.rs` `mouse_file_browser`, the `inside` computation uses
  `palette_overlay_rect(area, fb.entries.len())`; the painter and the row hit-test use
  `file_browser_overlay_rect(area, fb)`, which is `palette_overlay_rect(area, box_rows)`
  with `box_rows` including reserved footer rows (`chrome_geom.rs` `file_browser_rows`).
- **When they differ:** a destination picker with a non-empty field reserves one footer row
  (the resolved-target line), plus one per withholding disclosure. The drawn box is then
  1–2 rows taller than the click-away box. Probe p5 on 80x24 with an empty listing: drawn
  `height 4`, click-away `height 3`.
- **Effect:** a click on the drawn box's bottom border row (and, with a disclosure line, its
  last footer row) is classified as click-away. On the baseline that closed the picker and,
  for a quit-owned one, orphaned the quit (the I1 class). With the delta it now cancels the
  quit cleanly, so the delta strictly improves the consequence. The misclassification itself
  is outside this delta's scope and predates the branch (the `inside` block is untouched).
- **Disposition:** backlog item. The fix is to size `inside` from `file_browser_overlay_rect`
  so the click-away box is single-sourced with the painter, which is exactly the A21 hazard
  the ledger's doc comment describes. Not a blocker.

### N2 (Minor, docs) — `mouse_file_browser` doc comment does not mention the quit-owned cancel

- **Where:** `mouse.rs` doc comment above `mouse_file_browser` still reads "on a click-away
  closes the browser". The inline comment on the changed line does say it matches the
  registry closure for quit-owned pickers. Cosmetic; fold into the next touch of the file.

### Observation (not a finding)

The mouse cancel sets no status line, exactly like Esc (p4). The dialog disappearing is the
visible feedback, and the next Save and Quit restarts cleanly with fresh ownership and no
"in progress" warning (p3). Consistent with the design's Esc parity; noted only because the
project's "no silent UI" rule could reasonably be read to want a "quit cancelled" status on
both routes. If that is wanted, it belongs in `cancel_destination`, not in the mouse arm.

### Retained (explicitly deferred by the brief; not re-probed; code unchanged)

- Prior M2 — plugin-only closure of the quit-owned overwrite prompt via the registry row.
- Prior M3 — click-away on the close-buffer Save As picker leaves the stale CloseBuffer
  action (p1 confirms the delta did not change manual-picker closure).
- Prior M4/M5 and the fingerprint/summary UX observations.

## Evidence

| Check | Result |
| --- | --- |
| Manifest `source-sha256.json` | 26/26 match before; 26/26 match after restoring `lib.rs` |
| `followup.diff` vs live code | matches: one production line at `mouse.rs` click-away arm + one test |
| Focused durability suite | 33 passed (includes `mouse_click_away_cancels_a_quit_owned_filename_request`) |
| `cargo test --workspace` | 2,499 passed, 0 failed, 6 ignored across 18 summaries (prior 2,498 + the new test) |
| `cargo clippy --workspace --all-targets` | clean |
| `cargo build -p wordcartel` / `cargo test --workspace --no-run` | 0 warnings each |
| `scripts/smoke/run.sh` | `smoke: 9/9 PASS` (advisory; mouse input is not covered by the suite) |
| Fable probes (5) | 5 passed (`review-probes/fable_i1_probes.rs`) |

Probes, all through `app::reduce` with a synthetic left-button `MouseEvent`:

- p1 — manual `save_as` picker, click at (0,0): closes; a synthetic `CloseBuffer` action
  survives; no quit state. Delta did not broaden manual closure.
- p2 — quit-owned picker, clicks on every border cell plus title, field, and footer rows of
  the drawn box: picker stays open, `quit_save_owner`, `pending_save_as`, `quit_drain` intact.
- p3 — quit-owned picker, click-away, then `save_and_quit` again: all quit/pending fields
  cleared, `quit_drain_advance` false, fresh owned picker opens, no "in progress" warning.
- p4 — mouse click-away and `cancel_destination` (the Esc arm) produce identical end states.
- p5 — geometry: with a non-empty field the drawn box is one row taller than the click-away
  box; a click on the drawn footer's interior row is still inside and keeps ownership (N1).

## Limitations

- Mouse input was driven through `app::reduce`, not a real terminal; the smoke suite has no
  mouse checks.
- The second entry route to a quit-owned picker (Quit → Review Each → Save) was not driven
  by mouse here. Both routes set `quit_save_owner` through the same `open_save_as_picker`
  call, and the mouse arm reads only that field, so the reasoning covers it; the prior
  review's probe covered that route for the registry path.
- N1's disclosure-line case (2 reserved rows) was reasoned from `file_browser_rows`, not
  probed.
- No documents other than the brief, the diff, the logs, and the prior review were audited.

## Housekeeping

No production source was changed. `lib.rs` temporarily carried
`#[cfg(test)] #[path = "../../review-probes/fable_i1_probes.rs"] mod fable_i1_probes;` and
was restored (hash re-verified). Additions: `review-probes/fable_i1_probes.rs` and this
report. No rustfmt, commit, push, merge, delegation, credential access, or external contact.
The smoke run may have appended to the gitignored `scripts/smoke/.history`.
