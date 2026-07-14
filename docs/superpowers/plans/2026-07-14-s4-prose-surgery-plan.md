# S4 — Prose Surgery: implementation plan

**Effort:** S4 · **Arc:** S5 → S6 → **S4** → S7 → S8 · **Date:** 2026-07-14 · **Author:** Fable
**Spec (source of truth, Codex-clean 6 rounds):** `docs/superpowers/specs/2026-07-14-s4-prose-surgery-design.md`
**Process:** subagent-driven-development (fresh implementer PER TASK: failing test → run red → minimal
impl → run green → commit; then a per-task reviewer). Right-sized so a reviewer can reject one task
while approving its neighbor. Anchors are symbol NAMES (spec/plan `:NNN` may have drifted — re-locate
by name; verify signatures with `cargo`, not editor hints).

---

## Goal

Ship the surgery layer for the structure the S6 ventilate lens diagnoses: **N+M nullary commands** in a
leaf module — trustworthy grabbing (`select_section` + strict decline reusing the lens's own
`prose_block_at` at the CONTENT byte), the reorder (`move_sentence_up/down`, gap-preserving, caret+
selection travel, stop-at-edge), one object-agnostic `swap`, the ladder as a DATA TABLE with STATELESS
shrink (deletes `sel_history`, dissolving Hazard 4), joint surgery (`break_paragraph_here` /
`merge_paragraph_forward` / `split_sentence_at_caret`) with pinned gap-fate, counts (sentence-count in
the segment + `count_region`, all on ONE SP-7 helper), fold survival across move/swap (C-7/C-12/C-13),
and the B10 EOF-caret fix. All edits flow through `editor.apply`/`ChangeSet` (one undo unit).

## Architecture

Functional-core / imperative-shell. Core changes are minimal and additive: `wordcartel-core` gains
`textobj` re-use only (no new core types except `count::RegionStats` + `count::region_stats`). All
command/edit logic lives in the **`wordcartel` shell**: a new leaf module `commands/prose_ops.rs`
(A14 `commands/textops.rs` template — no `Command` variant, no `commands::run` arm; `registry.rs`
calls the handlers directly), plus a `Scope::Section` variant + a shared `prose_sentence_at` decline
predicate + the ladder data table in `commands.rs`, a `count::region_stats` SP-7 helper, a
`fold::corrected_after_move` fold-preservation helper wired into `blocks_marked::block_move` and the
new `swap`, and the B10 one-line clamp fix in `nav::caret_line`. SEE==SELECT is single-sourced: the
command path classifies at `ventilate::line_content_byte` (made `pub(crate)`) → `prose_block_at` →
`sentence_bounds` — the exact three calls the lens renders with.

## Tech stack

Rust; `wordcartel-core` (`#![forbid(unsafe_code)]`) + `wordcartel` shell. `ChangeSet`/`Transaction`/
`EditKind` edit substrate; `block_tree`/`outline`/`FoldState` structure; the registry/settings machinery.
Tests: in-crate `#[cfg(test)]` units; `TestBackend` cell inspection for lens/fold paint; `e2e.rs`
in-process `reduce → advance → render` journeys; `module_budgets` + palette-completeness invariants.

---

## Global constraints (binding — copied from CLAUDE.md / the spec)

- **House style, hand-formatted. Do NOT run `cargo fmt`** (no `rustfmt.toml`). Match neighbors:
  snake_case fns/vars/modules, PascalCase types, SCREAMING_SNAKE consts; 4-space indent; ~100-col
  hand-wrapped; imports grouped by hand; `—` em-dash in prose comments never `--`; no emoji in code.
  Do not reflow code you did not change.
- **`#![forbid(unsafe_code)]` governs `wordcartel-core`.** The only core change is `count::region_stats`
  + `RegionStats` — safe, allocation-light. No `unsafe` anywhere in S4.
- **Edits are CORE data-integrity and flow through `editor.apply(txn, edit, EditKind, clock)`** (the
  `submit_transaction`/`ChangeSet` channel) as ONE undo unit — the A14 template
  (`commands/textops.rs`): compute one `(from,to)`+replacement (or ascending non-overlapping edits via
  `build_multi_replace`), early-`Noop` on nothing-to-do, build `ChangeSet`+`block_tree::Edit`, `apply`,
  then `super::edit::settle_after_edit`.
- **SEE==SELECT (spec §8, C-11) — single-source.** `select_sentence` AND every caret-anchored mutation
  resolve the sentence via ONE helper `prose_sentence_at`, which classifies at the caret line's first
  non-whitespace CONTENT byte (`ventilate::line_content_byte`, made `pub(crate)`) → `prose_block_at`
  (classification + window) → `sentence_bounds`. NEVER classify at `line_start` (CommonMark strips
  ≤3-space indent → gap-fallback divergence). The `Scope::Sentence` arm of `scope_range_at` is
  refactored onto the same helper — no second sentence-resolution path.
- **Decline set (spec §3.3) — defined ONCE.** `prose_block_at` returns `None` iff `role_at(c) !=
  BlockRole::Paragraph`, so the decline set is EXACTLY the non-`Paragraph` `BlockRole`s: `Heading`,
  `BlockQuote`, `ListItem`, `CodeBlock`, `ThematicBreak`, `FrontMatter`, `Comment`. A **`Table` reads
  as PROSE** (no `Table` `BlockRole`; ratified = A) — consistent with the lens; true table-decline is
  **B14**, out of scope.
- **Two-region law (F2).** Exactly TWO region states: transient `Selection`, persistent `MarkedBlock`.
  S4 introduces NO third state ("current object" is never state — a select command PRODUCES a
  `Selection` and returns). `swap` reads the two existing regions; overlap **rejects LOUDLY** with a
  status message — never reach `build_multi_replace`'s silent identity-no-op guard.
- **Gap fate per site (spec §5.4).** M1 move: preserve the exact inter-sentence gap (`{B}{gap}{A}`).
  M2 swap: verbatim region bytes, outside whitespace untouched. M3 break: consume the single preceding
  separator, insert `"\n\n"`. M4 merge: replace the paragraph separator with ONE space. M5 split:
  insert `". "` (or `"."` if next char is whitespace), uppercase the next word initial. M6 generic
  cut: OUT — `select_sentence` is content-only (SEE==SELECT); gap-tidy is Effort-P Lua.
- **Stateless shrink (F4) deletes `sel_history` — the FULL compile census must be edited or the build
  breaks:** `editor.rs` field decl + init + clears in `Buffer::apply`/`undo`/`redo`; `commands.rs`
  `Move`/`SelectScope`/`SelectAll` arms + `ExpandSelection` push + `ShrinkSelection` pop; `marks.rs`
  (5 clears); `mouse.rs::seed_and_select` PUSH + its clear; `prompts.rs::goto_line` clear; and the
  tests (`editor.rs::apply_clears_sel_history`, undo-clear assert, `commands.rs` sel_history asserts,
  `mouse.rs` "seeds the expand ladder"). Removing the mouse push loses exact-range shrink-restore
  (F4's accepted cost); expand still grows from the current selection.
- **C-9 caret-at-start wiring.** `Selection::range(anchor, head)` puts the caret on the 2nd arg
  (`selection.rs`), so `set_selection_range` and every F8 command build `Selection::range(to, from)`
  (caret at START). Expand/shrink evaluate `scope_range_at` at the selection's `from()`.
- **C-7/C-12/C-13 fold correction (spec §7.3).** A moved/swapped folded section STAYS FOLDED: capture
  the folded anchors in each relocated region (relative) BEFORE the edit; compute the corrected fold
  set (moved anchors → destination geometry; stationary anchors → `change::map_pos` **After-bias** remap
  so a stationary heading at the destination advances PAST the inserted block — Codex-r4 Critical 2) as
  ONE set (`replace_folded` — no per-region interleaving, C-12); sequence apply → rebuild (settle) →
  `replace_folded(corrected)` → rebuild (relayout) so the view reflects corrected folds (C-13).
- **Anti-regrowth (spec §10) is a GATE.** New handlers live in the leaf `commands/prose_ops.rs`;
  new commands are registry data rows; the ladder is a `const LADDER` data table. NO growth in
  `reduce`/`place_cursor`/hub dispatchers. `clippy::too_many_lines` (100) and
  `wordcartel/tests/module_budgets.rs` are GATEs — re-check after Tasks 3/8.
- **Command-surface-contract conformance is a merge GATE** (§ below). Palette-completeness +
  `every_persisted_setting_has_a_command` (N/A here — no persisted option) stay green.
- **Workspace clippy `all = "deny"` is a GATE.** `cargo clippy --workspace --all-targets` clean; a
  deliberate exception needs an item-local `#[allow(clippy::…)]` + one-line rationale.
- **PTY smoke** (`scripts/smoke/run.sh`) mandatory-run / advisory-pass; the pre-merge report quotes its
  one-line summary.
- **rust-analyzer lags edits.** Trust `cargo build`/`check`/`clippy`/`test` + `grep`, not editor hints.
- **Every commit ends with the project trailers, verbatim:**
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_018zpBg3F9gzJKpejo6JSHDG
  ```
- **Exact new command ids / labels / menu (LAW — do not rename):**
  | id | label | menu | routing |
  |---|---|---|---|
  | `select_section` | `Select Section` | None | `run(c, Command::SelectScope(Scope::Section))` |
  | `move_sentence_up` | `Move Sentence Up` | Edit | `prose_ops::move_sentence_up(c.editor, c.clock)` |
  | `move_sentence_down` | `Move Sentence Down` | Edit | `prose_ops::move_sentence_down(c.editor, c.clock)` |
  | `swap` | `Swap Selection ⇄ Block` | Block | `prose_ops::swap(c.editor, c.clock)` |
  | `break_paragraph_here` | `Break Paragraph Here` | Edit | `prose_ops::break_paragraph_here(c.editor, c.clock)` |
  | `merge_paragraph_forward` | `Merge Paragraph Forward` | Edit | `prose_ops::merge_paragraph_forward(c.editor, c.clock)` |
  | `split_sentence_at_caret` | `Split Sentence` | Edit | `prose_ops::split_sentence_at_caret(c.editor, c.clock)` |
  | `count_region` | `Count Region` | View | `prose_ops::count_region(c.editor)` |

  (`select_sentence`/`select_paragraph`/`expand_selection`/`shrink_selection` already exist — S4
  modifies their internals, not registration.)

---

## File structure

**New module:**
- `wordcartel/src/commands/prose_ops.rs` — the eight leaf handlers: `move_sentence_up`,
  `move_sentence_down`, `swap`, `break_paragraph_here`, `merge_paragraph_forward`,
  `split_sentence_at_caret`, `count_region` (+ small private helpers). Declared `pub(crate) mod
  prose_ops;` in `commands.rs` beside `pub(crate) mod textops;`.

**Existing files touched:**
- `wordcartel-core/src/count.rs` — `RegionStats` struct + `region_stats(&str) -> RegionStats` (SP-7).
- `wordcartel/src/ventilate.rs` — `line_content_byte`: `fn` → `pub(crate) fn` (single-source).
- `wordcartel/src/commands.rs` — `Scope::Section`; `scope_range_at` `Section` arm; `prose_sentence_at`
  + `NonProse` + `block_kind_label`; `section_range_at`; `select_scope_or_decline`; `set_selection_range`
  head-at-start; `ExpandSelection`/`ShrinkSelection` rewritten (data table + stateless); `SelectScope`
  decline routing; delete `sel_history` touch-points; `pub(crate) mod prose_ops;`.
- `wordcartel/src/editor.rs` — delete `sel_history` field + init + the three `.clear()` sites.
- `wordcartel/src/marks.rs` — delete the five `sel_history.clear()` calls.
- `wordcartel/src/mouse.rs` — delete `seed_and_select`'s `sel_history.push` + the clear + the test.
- `wordcartel/src/prompts.rs` — delete `goto_line`'s `sel_history.clear()`.
- `wordcartel/src/registry.rs` — the 8 new command rows (data-table growth).
- `wordcartel/src/render_status.rs` — `word_count_segment` uses `count::region_stats` (adds sentences).
- `wordcartel/src/fold.rs` — `corrected_after_move(folds, regions, cs) -> BTreeSet<usize>`.
- `wordcartel/src/blocks_marked.rs` — `block_move` gains the fold capture/replace/rebuild bracket.
- `wordcartel/src/nav.rs` — `caret_line` B10 clamp fix.
- `wordcartel/src/e2e.rs` (test) — the lens+surgery journey (Task 10).

> **Module declaration:** the leaf module is declared in `commands.rs` (`pub(crate) mod prose_ops;`),
> mirroring `pub(crate) mod textops;` — NOT in `lib.rs`.

---

## Command-surface-contract conformance (merge GATE)

- **LAW 1 (registry SSOT).** Every capability is a registered command; every edit goes through
  `editor.apply` (`ChangeSet`).
- **LAW 2 (every user-settable option is a command).** **N/A — S4 adds NO user-settable option** (no
  `SettingsSnapshot`/`OView`/config field, no setter). `every_persisted_setting_has_a_command` untouched.
- **LAW 3 (palette exhaustive).** All 8 new commands are non-hidden → auto-listed; palette-completeness
  enforces. `swap` has both a palette row and a Block-menu row; no state reachable only by a non-palette
  door.
- **LAW 4 (menu ⊆ palette).** Curated menu subset: the 5 Edit-menu edit commands, `swap` (Block),
  `count_region` (View). `select_section` palette-only (matches `select_sentence`). All in the palette.
- **LAW 5 (mouse affordance ⇒ keyboard path).** N/A — no new mouse affordance.
- **LAW 6 (one setter per option).** N/A — no option. (`region_stats` is a shared READER — SP-7's
  one-source discipline for stats.)
- **RULE 8 (multi-state option shape).** N/A — `Scope::Section` is an internal enum variant, not a user
  option; expand/shrink are single-action commands, not a state cycle.
- **LAW 7 (hints track keymap).** No default chord → hint re-resolution inherited for free;
  `hints_reresolve_on_preset_switch` unaffected.
- **RULE 10 (plugin/automation spine — NO amendment).** All new commands NULLARY. The object×operator
  cross-product ("delete sentence" = `select_sentence` then `cut`) is Effort-P Lua — no law-10
  amendment. `swap` reads the two existing regions → nullary.

---

## Task list (each = one implementer subagent, TDD, commit at end)

Order flows forward on shipped interfaces: **T1** shared decline predicate + `line_content_byte`
`pub(crate)` + `Scope::Sentence` refactor → **T2** `Scope::Section` + `select_section` + head-at-start
wiring → **T3** ladder data table + stateless shrink (delete `sel_history`) → **T4** SP-7 count helper
+ segment + `count_region` → **T5** `move_sentence_up/down` → **T6** two-region `swap` → **T7** joint
edits → **T8** fold correction wired into `swap` + `block_move` (AFTER both exist) → **T9** B10 caret
clamp → **T10** e2e journey + budgets. T5–T7 depend on T1. T8 depends on T6 + the shipped `block_move`.

---

### Task 1 — SEE==SELECT decline predicate + `line_content_byte` pub(crate) + `Scope::Sentence` refactor

**Deliverable:** the single sentence-resolution helper `prose_sentence_at` (content-byte classification,
strict decline), `line_content_byte` made `pub(crate)`, and `select_sentence` + `scope_range_at`'s
`Sentence` arm both routed through it. No behavior change on prose; a heading/list/code/blockquote/
front-matter/comment caret now DECLINES; a table reads as prose; indented prose no longer diverges.

**Files:**
- modify `wordcartel/src/ventilate.rs` (visibility)
- modify `wordcartel/src/commands.rs` (helper + refactor + `SelectScope` decline routing)
- test in `wordcartel/src/commands.rs` `#[cfg(test)] mod tests`

**Interfaces — Produces:**
- `ventilate::line_content_byte(buf: &TextBuffer, l: usize) -> Option<usize>` — now `pub(crate)`.
- `commands::prose_sentence_at(editor: &Editor, h: usize) -> Result<(usize, usize), NonProse>` where
  `struct NonProse(pub wordcartel_core::style::BlockRole)`.
- `commands::block_kind_label(role: BlockRole) -> &'static str`.

**Consumes:** `ventilate::prose_block_at`, `textobj::sentence_bounds`, `derive::line_start`,
`block_tree::role_at`.

**TDD steps.**

1. **Failing test** — add to `commands.rs` tests (`use super::*;` is already present; the module opens
   with `struct TestClock` etc.):
   ```rust
   #[test]
   fn prose_sentence_at_declines_non_prose_and_resolves_prose() {
       // Prose: caret in a paragraph resolves the sentence.
       let mut e = Editor::new_from_text("One two. Three four.\n", None, (40, 12));
       derive::rebuild(&mut e);
       assert_eq!(prose_sentence_at(&e, 2), Ok((0, 8)));   // "One two."
       // Heading: declines (Heading role).
       let mut h = Editor::new_from_text("# Title\n\nbody text.\n", None, (40, 12));
       derive::rebuild(&mut h);
       assert!(matches!(prose_sentence_at(&h, 2), Err(NonProse(_))));
       // The paragraph after the heading still resolves.
       let body = "# Title\n\nbody text.\n".find("body").unwrap();
       assert!(prose_sentence_at(&h, body).is_ok());
       // Indented prose (CommonMark strips ≤3-space indent): content-byte classification, NOT decline.
       let mut ind = Editor::new_from_text("  Indented one. Indented two.\n", None, (40, 12));
       derive::rebuild(&mut ind);
       assert!(prose_sentence_at(&ind, 5).is_ok(), "indented prose classifies via content byte");
   }
   ```
   **Run (pre-impl):** `cargo test -p wordcartel prose_sentence_at_declines_non_prose_and_resolves_prose`
   → **compile error** (`prose_sentence_at`/`NonProse` absent).

2. **Impl — `line_content_byte` visibility** (`ventilate.rs`): change the signature
   `fn line_content_byte(buf: &TextBuffer, l: usize) -> Option<usize>` to
   `pub(crate) fn line_content_byte(buf: &TextBuffer, l: usize) -> Option<usize>` (body unchanged —
   it returns `line_start + first-non-whitespace offset`, or `None` for a blank line).

3. **Impl — the helper** (`commands.rs`, near `scope_range_at`):
   ```rust
   /// The `BlockRole` returned when a caret is not in prose — carried so the command can name the
   /// block kind in its decline message (F3).
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub struct NonProse(pub wordcartel_core::style::BlockRole);

   /// Human name of a non-`Paragraph` block role, for the "no sentence here (…)" message.
   pub fn block_kind_label(role: wordcartel_core::style::BlockRole) -> &'static str {
       use wordcartel_core::style::BlockRole::*;
       match role {
           Paragraph => "paragraph", Heading(_) => "heading", BlockQuote => "block quote",
           ListItem => "list item", CodeBlock => "code block", ThematicBreak => "rule",
           FrontMatter => "front matter", Comment => "comment",
       }
   }

   /// The sentence scope at byte `h`, via the LENS'S OWN classification + window, or `Err(NonProse)`
   /// when `h` is not in prose. SEE==SELECT single-source (spec §8, C-11): classify at the caret line's
   /// first non-whitespace CONTENT byte (`ventilate::line_content_byte`) — CommonMark strips ≤3-space
   /// block indent, so a `line_start` classification would hit the gap fallback and diverge from the
   /// lens on indented prose. Then `prose_block_at` (window) + `sentence_bounds` (segmentation) — the
   /// exact calls the lens renders with.
   pub fn prose_sentence_at(editor: &Editor, h: usize) -> Result<(usize, usize), NonProse> {
       let b = editor.active();
       let buf = &b.document.buffer;
       let blocks = b.document.blocks();
       let line = buf.byte_to_line(h.min(buf.len()));
       let Some(c) = crate::ventilate::line_content_byte(buf, line) else {
           return Err(NonProse(blocks.role_at(crate::derive::line_start(buf, line))));
       };
       match crate::ventilate::prose_block_at(blocks, buf, c) {
           None => Err(NonProse(blocks.role_at(c))),
           Some((ps, pe)) => {
               let rel = h.saturating_sub(ps);
               let (sf, st) = wordcartel_core::textobj::sentence_bounds(&buf.slice(ps..pe), rel);
               Ok((ps + sf, ps + st))
           }
       }
   }
   ```

4. **Impl — refactor `scope_range_at`'s `Sentence` arm** onto the helper (drop the inline duplication).
   Current arm:
   ```rust
   Scope::Sentence => {
       let (ps, pe) = nav::paragraph_range_at(blocks, buf, h);
       let win = buf.slice(ps..pe);
       let (sf, st) = wordcartel_core::textobj::sentence_bounds(&win, h - ps);
       (ps + sf, ps + st)
   }
   ```
   becomes:
   ```rust
   Scope::Sentence => prose_sentence_at(editor, h).unwrap_or((h, h)), // decline → empty → ladder skips
   ```
   (`scope_range_at` stays total: an empty `(h,h)` on decline makes the ladder's strict-containment
   test fail, so a non-prose Sentence rung is skipped — spec §3.4.)

5. **Impl — `select_sentence` decline routing.** In `commands::run`, the `SelectScope(scope)` arm
   currently does `let (from,to) = scope_range(editor, scope); set_selection_range(editor, from, to)`.
   Add a decline branch for `Sentence` (Section is added in T2):
   ```rust
   Command::SelectScope(scope) => {
       editor.active_mut().sel_history.clear(); // (removed in T3)
       match scope {
           Scope::Sentence => match prose_sentence_at(editor, nav::head(editor)) {
               Ok((from, to)) => { set_selection_range(editor, from, to); CommandResult::Handled }
               Err(NonProse(role)) => {
                   editor.status = format!("no sentence here ({})", block_kind_label(role));
                   CommandResult::Noop
               }
           },
           _ => {
               let (from, to) = scope_range(editor, scope);
               set_selection_range(editor, from, to);
               CommandResult::Handled
           }
       }
   }
   ```

6. **Green:** `cargo test -p wordcartel prose_sentence_at_declines_non_prose_and_resolves_prose` and the
   existing `select_sentence`/ladder tests → pass. `cargo build -p wordcartel` clean; clippy clean.

7. **Commit:** `S4 T1: SEE==SELECT prose_sentence_at decline predicate + content-byte classification` (+ trailers).

**Note to implementer:** the `set_selection_range` head-at-start change is **T2's** — leave it
`Selection::range(from, to)` here; T1's `select_sentence` test asserts the RANGE, not the caret end.

---

### Task 2 — `Scope::Section` + `select_section` + head-at-start selection wiring (C-9)

**Deliverable:** the `Section` scope (deepest enclosing heading subtree, `heading.byte .. body.end`,
declines when no enclosing heading), the `select_section` command, and the C-9 head-at-start wiring in
`set_selection_range` (caret at span START for every ladder select + F8 command).

**Files:**
- modify `wordcartel/src/commands.rs` (`Scope`, `scope_range_at`, `section_range_at`, `SelectScope`
  Section branch, `set_selection_range`)
- modify `wordcartel/src/registry.rs` (`select_section` row)
- test in `commands.rs` tests

**Interfaces — Produces:**
- `commands::Scope::Section` variant.
- `commands::section_range_at(editor: &Editor, h: usize) -> Option<(usize, usize)>`.
- registry command `select_section`.

**Consumes:** `outline::sections`, `outline::Section`, `buf.snapshot()`.

**TDD steps.**

1. **Failing test** (`commands.rs` tests):
   ```rust
   #[test]
   fn select_section_selects_subtree_caret_at_start() {
       let doc = "# Title\n\nintro para.\n\n## A\n\nbody of a.\n\n### A1\n\ninner.\n";
       let mut e = Editor::new_from_text(doc, None, (60, 20));
       derive::rebuild(&mut e);
       // caret inside "body of a." → deepest containing section is "## A" (its subtree includes A1).
       let at = doc.find("body of a.").unwrap();
       let a_start = doc.find("## A").unwrap();
       let sec = section_range_at(&e, at).expect("a section here");
       assert_eq!(sec.0, a_start, "section starts at the ## A heading byte");
       assert_eq!(sec.1, doc.len(), "## A subtree runs to EOF (includes ### A1)");
       // caret inside "inner." → deepest is "### A1".
       let inner = doc.find("inner.").unwrap();
       let a1_start = doc.find("### A1").unwrap();
       assert_eq!(section_range_at(&e, inner).unwrap().0, a1_start);
       // select_section (via the direct run() path — no registry needed) sets the selection
       // head-at-start (C-9).
       e.active_mut().document.selection = Selection::single(at);
       assert_eq!(run(Command::SelectScope(Scope::Section), &mut e, &TestClock(0)), CommandResult::Handled);
       let pr = e.active().document.selection.primary();
       assert_eq!((pr.from(), pr.to()), (a_start, doc.len()));
       assert_eq!(pr.head, a_start, "C-9: caret at the section START");
   }
   #[test]
   fn select_section_declines_when_no_heading() {
       let mut e = Editor::new_from_text("just a paragraph, no headings.\n", None, (50, 12));
       derive::rebuild(&mut e);
       assert_eq!(run(Command::SelectScope(Scope::Section), &mut e, &TestClock(0)), CommandResult::Noop);
   }
   ```
   (These use the direct `run(Command::SelectScope(Scope::Section), &mut e, &TestClock(0))` path — the
   `commands.rs` test convention — so no registry `dispatch` helper is needed. The registry ROW is
   covered by the palette-completeness test. `run`/`Command`/`Scope`/`TestClock` are all in scope in
   `commands.rs` tests.)
   **Run (pre-impl):** `cargo test -p wordcartel select_section` → **compile error** (`Scope::Section`,
   `section_range_at`, the command absent).

2. **Impl — `Scope::Section` + `scope_range_at` arm** (`commands.rs`):
   ```rust
   pub enum Scope { Word, Sentence, Paragraph, Section, Document } // add Section
   ```
   ```rust
   /// The DEEPEST (smallest) heading subtree `[heading.byte, body.end)` containing `h`, or `None` when
   /// `h` is under no heading. `outline::sections` yields nested ranges; the deepest-containing one is
   /// the innermost enclosing scene (spec §3.1). Cold path: O(headings)+alloc, command/expand-press
   /// triggered — never per-keystroke (C-3).
   pub fn section_range_at(editor: &Editor, h: usize) -> Option<(usize, usize)> {
       let b = editor.active();
       let rope = b.document.buffer.snapshot();
       wordcartel_core::outline::sections(b.document.blocks(), &rope)
           .into_iter()
           .filter(|s| s.heading.byte <= h && h < s.body.end)
           .min_by_key(|s| s.body.end - s.heading.byte)
           .map(|s| (s.heading.byte, s.body.end))
   }
   ```
   Add the `scope_range_at` arm:
   ```rust
   Scope::Section => section_range_at(editor, h).unwrap_or((h, h)), // decline → empty → ladder skips
   ```

3. **Impl — head-at-start wiring (C-9)** in `set_selection_range`:
   ```rust
   fn set_selection_range(editor: &mut Editor, from: usize, to: usize) {
       // C-9: head-at-start — Selection::range(anchor, head) puts the caret on the 2nd arg, so pass
       // (to, from) → from()==from, to()==to, head==from. The caret lands at the span START (F8), and
       // expand/shrink evaluate scope_range_at from inside the span.
       editor.active_mut().document.selection = Selection::range(to, from);
       derive::rebuild(editor);
       nav::ensure_visible(editor);
   }
   ```
   > This changes the shipped caret landing of `select_word`/`select_sentence`/`select_paragraph` from
   > the selection END to its START (deliberate — spec C-9, ratified = A). Grep `select_word`/
   > `select_sentence`/`select_paragraph` tests in `commands.rs`/`registry.rs`; update any that asserted
   > the old end-landing (assert `pr.head == pr.from()`).

4. **Impl — `SelectScope` Section decline branch** (extend T1's match):
   ```rust
   Scope::Section => match section_range_at(editor, nav::head(editor)) {
       Some((from, to)) => { set_selection_range(editor, from, to); CommandResult::Handled }
       None => { editor.status = "no section here".into(); CommandResult::Noop }
   },
   ```

5. **Impl — registry row** (`registry.rs`, beside `select_paragraph`):
   ```rust
   r.register("select_section", "Select Section", None, |c| run(c, Command::SelectScope(Scope::Section)));
   ```

6. **Green:** `cargo test -p wordcartel select_section` + the updated select_* caret tests + palette
   completeness → pass. `cargo build`/`clippy -p wordcartel` clean.

7. **Commit:** `S4 T2: Scope::Section + select_section + head-at-start selection (C-9)` (+ trailers).

---

### Task 3 — Ladder as data table + stateless shrink + delete `sel_history` (Hazard 4)

**Deliverable:** the expand/shrink ladder as a `const LADDER` data table including the Section rung;
STATELESS shrink (re-derives the largest scope strictly contained, at `from()`); the entire
`sel_history` field deleted across the FULL compile census. Dissolves Hazard 4.

**Files:**
- modify `wordcartel/src/commands.rs` (`LADDER`, `ExpandSelection`/`ShrinkSelection` arms, remove
  `sel_history` refs in `Move`/`SelectScope`/`SelectAll`)
- modify `wordcartel/src/editor.rs` (delete field + init + 3 clears; delete 2 tests)
- modify `wordcartel/src/marks.rs` (delete 5 clears)
- modify `wordcartel/src/mouse.rs` (delete push + clear + test)
- modify `wordcartel/src/prompts.rs` (delete clear)
- tests in `commands.rs`

**Interfaces — Produces:** `Command::ExpandSelection`/`ShrinkSelection` behavior unchanged in name,
now data-driven + stateless. **Removes:** `Buffer.sel_history`.

**Consumes:** `scope_range_at` incl. `Scope::Section` (T2).

**TDD steps.**

1. **Failing tests** (`commands.rs`):
   ```rust
   #[test]
   fn ladder_expands_word_sentence_paragraph_section_document() {
       let doc = "# H\n\nOne two. Three four.\n";
       let mut e = Editor::new_from_text(doc, None, (60, 12));
       derive::rebuild(&mut e);
       let at = doc.find("two").unwrap();
       e.active_mut().document.selection = Selection::single(at);
       run(Command::SelectScope(Scope::Word), &mut e, &TestClock(0));      // "two"
       run(Command::ExpandSelection, &mut e, &TestClock(0));               // → Sentence "One two."
       let p = e.active().document.selection.primary();
       assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "One two.");
       run(Command::ExpandSelection, &mut e, &TestClock(0));               // → Paragraph
       run(Command::ExpandSelection, &mut e, &TestClock(0));               // → Section (# H subtree)
       let s = e.active().document.selection.primary();
       assert_eq!(s.from(), doc.find("# H").unwrap());
       run(Command::ExpandSelection, &mut e, &TestClock(0));               // → Document
       let d = e.active().document.selection.primary();
       assert_eq!((d.from(), d.to()), (0, doc.len()));
   }
   #[test]
   fn stateless_shrink_returns_first_sentence_and_survives_undo() {
       let doc = "One two. Three four.\n";
       let mut e = Editor::new_from_text(doc, None, (40, 12));
       derive::rebuild(&mut e);
       e.active_mut().document.selection = Selection::single(2);
       run(Command::SelectScope(Scope::Paragraph), &mut e, &TestClock(0));
       run(Command::ShrinkSelection, &mut e, &TestClock(0)); // paragraph → FIRST sentence (from())
       let p = e.active().document.selection.primary();
       assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "One two.");
       // Hazard-4: expand → edit → shrink must not panic and must yield a canonical rung.
       run(Command::SelectScope(Scope::Paragraph), &mut e, &TestClock(0));
       run(Command::InsertChar('!'), &mut e, &TestClock(1));
       e.undo();
       run(Command::ShrinkSelection, &mut e, &TestClock(2)); // no panic, no stale state
   }
   ```
   **Run (pre-impl):** these pass-or-fail on the NEW behavior; but the task's RED state is the compile
   break from deleting `sel_history` — run `cargo build -p wordcartel` first and expect the census
   errors; the two behavior tests then drive the rewritten arms.

2. **Impl — the data table + arms** (`commands.rs`). Replace the inline `order` array and the
   history-based arms:
   ```rust
   /// Expand/shrink rungs, finest → coarsest. A data table (spec §3.4) — adding a rung is a table
   /// edit, not dispatcher growth. A declined scope (`Sentence` on non-prose, `Section` with no
   /// enclosing heading) yields an empty range and is skipped by the strict-containment test.
   const LADDER: &[Scope] = &[Scope::Word, Scope::Sentence, Scope::Paragraph, Scope::Section, Scope::Document];
   ```
   `ExpandSelection` arm (evaluate at `from()`, no push):
   ```rust
   Command::ExpandSelection => {
       let cur = editor.active().document.selection.primary();
       let (cf, ct) = (cur.from(), cur.to());
       let from = cf; // C-9: evaluate strictly inside the span
       let mut next: Option<(usize, usize)> = None;
       for sc in LADDER.iter().copied() {
           let (f, t) = scope_range_at(editor, from, sc);
           if f <= cf && t >= ct && (f < cf || t > ct) { next = Some((f, t)); break; }
       }
       match next {
           Some((f, t)) => { set_selection_range(editor, f, t); CommandResult::Handled }
           None => CommandResult::Noop,
       }
   }
   ```
   `ShrinkSelection` arm (STATELESS — largest strictly-contained, coarsest→finest):
   ```rust
   Command::ShrinkSelection => {
       let cur = editor.active().document.selection.primary();
       let (cf, ct) = (cur.from(), cur.to());
       let from = cf;
       let mut inner: Option<(usize, usize)> = None;
       for sc in LADDER.iter().rev().copied() {
           let (f, t) = scope_range_at(editor, from, sc);
           if f >= cf && t <= ct && (f > cf || t < ct) && f < t { inner = Some((f, t)); break; }
       }
       match inner {
           Some((f, t)) => { set_selection_range(editor, f, t); CommandResult::Handled }
           None => CommandResult::Noop,
       }
   }
   ```

3. **Impl — delete `sel_history` (full census).** Remove:
   - `editor.rs`: the field `pub sel_history: Vec<...>,`; its `sel_history: Vec::new(),` init; the three
     `self.sel_history.clear();` lines in `Buffer::apply`/`undo`/`redo`; the tests
     `apply_clears_sel_history` and the undo-clear assertion (`sel_history` asserts in the undo/redo
     test) — delete those `sel_history` lines/tests.
   - `commands.rs`: the `editor.active_mut().sel_history.clear();` in the `Move` arm, the `SelectScope`
     arm (T1 added one — remove it), and `SelectAll`; and any `sel_history` asserts in commands tests.
   - `marks.rs`: the five `sel_history.clear()` calls (`set_mark` and the four others).
   - `mouse.rs`: in `seed_and_select`, delete the `let cur_sel = ...; editor.active_mut()
     .sel_history.push(cur_sel);` lines (keep the `Selection::range(f, t)` set + rebuild + ensure_visible);
     delete the `sel_history.clear()` at the other mouse site; delete the `..._seeds the expand ladder`
     test.
   - `prompts.rs`: the `sel_history.clear()` in `goto_line`.
   Grep to confirm zero remaining refs: `grep -rn sel_history wordcartel/src` must return nothing.

4. **Green:** `cargo build -p wordcartel` clean (census complete); `cargo test -p wordcartel
   ladder_expands_word_sentence_paragraph_section_document
   stateless_shrink_returns_first_sentence_and_survives_undo` → pass. `cargo test -p wordcartel
   --test module_budgets` clean. clippy clean.

5. **Commit:** `S4 T3: ladder data table + stateless shrink; delete sel_history (Hazard 4)` (+ trailers).

---

### Task 4 — SP-7 count helper + segment sentence-count + `count_region`

**Deliverable:** ONE core `count::region_stats`; `render_status::word_count_segment` extended with a
sentence count; the `count_region` command posting the fuller readout.

**Files:**
- modify `wordcartel-core/src/count.rs` (`RegionStats` + `region_stats`)
- modify `wordcartel/src/render_status.rs` (`word_count_segment`)
- create `wordcartel/src/commands/prose_ops.rs` (with `count_region`; module also hosts T5–T7)
- modify `wordcartel/src/commands.rs` (`pub(crate) mod prose_ops;`)
- modify `wordcartel/src/registry.rs` (`count_region` row)
- tests in `count.rs`, `render_status.rs`, `prose_ops.rs`

**Interfaces — Produces:**
- `count::RegionStats { pub words: usize, pub sentences: usize, pub chars: usize }`.
- `count::region_stats(text: &str) -> RegionStats`.
- `prose_ops::count_region(editor: &mut Editor) -> CommandResult`.

**Consumes:** `textobj::sentence_spans`, `count::word_count`, `count::char_count`.

**TDD steps.**

1. **Failing test — core** (`count.rs` tests):
   ```rust
   #[test]
   fn region_stats_words_sentences_chars() {
       let s = region_stats("One two. Three four five.");
       assert_eq!(s.words, 5);
       assert_eq!(s.sentences, 2);
       assert_eq!(s.chars, "One two. Three four five.".chars().count());
       let z = region_stats("");
       assert_eq!((z.words, z.sentences, z.chars), (0, 0, 0));
   }
   ```
   **Run:** `cargo test -p wordcartel-core region_stats_words_sentences_chars` → compile error.

2. **Impl — core** (`wordcartel-core/src/count.rs`, after `char_count`):
   ```rust
   /// Words, sentences, and chars over one text window — the SP-7 shared stats helper. Sentences via
   /// `crate::textobj::sentence_spans` (content-only); words/chars via the existing counters. The
   /// status segment, the S6 gutter, and `count_region` all route through this ONE helper.
   #[derive(Debug, Clone, Copy, PartialEq, Eq)]
   pub struct RegionStats { pub words: usize, pub sentences: usize, pub chars: usize }

   pub fn region_stats(text: &str) -> RegionStats {
       RegionStats {
           words: word_count(text),
           sentences: crate::textobj::sentence_spans(text).count(),
           chars: char_count(text),
       }
   }
   ```
   (Free fn only — the Produces interface. No `RegionStats::of` constructor; `word_count`/`char_count`
   stay the shared per-piece primitives the gutter reuses — step 5.)

3. **Impl — segment** (`render_status.rs::word_count_segment`): replace the `format!` body to use
   `region_stats`:
   ```rust
   let st = count::region_stats(&text);
   Some(format!("{} words · {} sentences · {} chars", st.words, st.sentences, st.chars))
   ```
   (Keep the existing selection-vs-buffer `text` selection and the `view_opts.word_count` gate.)
   Update `word_count_segment_selection_aware` to expect the new "N words · N sentences · N chars"
   string.

3b. **SP-7 third consumer — the S6 gutter (Codex finding 4).** Spec §6.1: all three consumers converge
   on the shared `wordcartel-core::count` home. The gutter `ventilate::layout_block` already calls
   `wordcartel_core::count::word_count(&raw[sf..st])` per sentence row — and `region_stats` is BUILT on
   that same `word_count`, so there is exactly ONE word-counting implementation (no duplicate). Per
   spec §6.1 the gutter **keeps** its per-span `word_count` call (using the full `region_stats` per
   visible row would add a needless `sentence_spans`+`char_count` pass per row — the gutter needs only
   the word count). This step is a CONFIRMATION + a guard test, not a gutter rewrite:
   - `grep -n "count::word_count\|count::char_count\|region_stats" wordcartel/src wordcartel-core/src`
     → the ONLY word/char counting is in `wordcartel-core::count`; the gutter, the segment, and
     `count_region` all call into it (segment + count_region via `region_stats`, gutter via
     `word_count`). No count logic lives outside `count.rs`.
   - Add a guard test (`count.rs` tests) pinning that the gutter's per-sentence word count agrees with
     the shared helper:
     ```rust
     #[test]
     fn region_stats_words_matches_word_count_for_a_single_sentence() {
         // The gutter uses count::word_count per sentence; region_stats.words must agree (one source).
         let s = "The committee met on Tuesday.";
         assert_eq!(region_stats(s).words, word_count(s));
     }
     ```

4. **Impl — leaf module + `count_region`.** Create `wordcartel/src/commands/prose_ops.rs`:
   ```rust
   //! S4 prose-surgery commands — a leaf module on the A14 template (no `Command` variant, no
   //! `commands::run` arm; `registry.rs` calls these directly). Edits flow through `editor.apply`
   //! (`ChangeSet`) as one undo unit. SEE==SELECT + decline route through `super::prose_sentence_at`.

   use crate::editor::Editor;
   use super::CommandResult;

   /// `count_region` — post "N words · N sentences · N chars" for the current region (selection if
   /// non-empty, else the whole buffer) to the status line. Pure report; no mutation.
   pub(crate) fn count_region(editor: &mut Editor) -> CommandResult {
       let sel = editor.active().document.selection.primary();
       let text = if !sel.is_empty() {
           editor.active().document.buffer.slice(sel.from()..sel.to())
       } else {
           editor.active().document.buffer.to_string()
       };
       let st = wordcartel_core::count::region_stats(&text);
       editor.status = format!("{} words · {} sentences · {} chars", st.words, st.sentences, st.chars);
       CommandResult::Handled
   }
   ```
   Add `pub(crate) mod prose_ops;` to `commands.rs` beside `pub(crate) mod textops;`.

5. **Impl — registry row** (`registry.rs`, View menu band):
   ```rust
   r.register("count_region", "Count Region", Some(MenuCategory::View),
       |c| crate::commands::prose_ops::count_region(c.editor));
   ```

6. **Failing/green test — `count_region`** (`prose_ops.rs` tests):
   ```rust
   #[cfg(test)]
   mod tests {
       use super::*;
       #[test]
       fn count_region_reports_selection_then_buffer() {
           let mut e = Editor::new_from_text("One two. Three four.\n", None, (40, 12));
           crate::derive::rebuild(&mut e);
           count_region(&mut e);
           assert!(e.status.contains("2 sentences"), "buffer: {}", e.status);
           e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 8);
           count_region(&mut e);
           assert!(e.status.contains("1 sentences") && e.status.contains("2 words"), "sel: {}", e.status);
       }
   }
   ```

7. **Green:** `cargo test -p wordcartel-core region_stats_words_sentences_chars
   region_stats_words_matches_word_count_for_a_single_sentence`, `cargo test -p wordcartel
   count_region_reports_selection_then_buffer word_count_segment_selection_aware` → pass.
   Palette-completeness green. `cargo build`/`clippy --workspace` clean.

8. **Commit:** `S4 T4: SP-7 region_stats helper + segment sentence-count + count_region` (+ trailers).

---

### Task 5 — `move_sentence_up` / `move_sentence_down` (gap-preserving, caret travels, stop-at-edge)

**Deliverable:** the reorder handlers — swap the caret's sentence with its neighbor within the
paragraph window, PRESERVING the inter-sentence gap; caret+selection land on the MOVED sentence
(head-at-start); STOP at the paragraph edge with a status message; decline on non-prose.

**Files:** modify `wordcartel/src/commands/prose_ops.rs` (+ tests); modify `registry.rs` (2 rows).

**Interfaces — Produces:** `prose_ops::move_sentence_up(&mut Editor, &dyn Clock) -> CommandResult`,
`prose_ops::move_sentence_down(&mut Editor, &dyn Clock) -> CommandResult`.

**Consumes:** `super::prose_sentence_at` (via the window it computes), `textobj::sentence_spans`,
`super::build_range_replace`, `super::edit::settle_after_edit`, `nav::head`, `nav::paragraph_range_at`.

**TDD steps.**

1. **Failing tests** (`prose_ops.rs` tests):
   ```rust
   #[test]
   fn move_sentence_down_swaps_preserving_gap_caret_travels() {
       let mut e = Editor::new_from_text("Alpha one. Beta two. Gamma three.\n", None, (60, 12));
       crate::derive::rebuild(&mut e);
       // caret in "Alpha one."
       e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2);
       assert_eq!(move_sentence_down(&mut e, &TestClock(0)), CommandResult::Handled);
       assert_eq!(e.active().document.buffer.to_string(), "Beta two. Alpha one. Gamma three.\n");
       // caret+selection now on the MOVED sentence "Alpha one." at its new position, head at start.
       let p = e.active().document.selection.primary();
       assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "Alpha one.");
       assert_eq!(p.head, p.from());
       // repeat moves the SAME sentence again.
       assert_eq!(move_sentence_down(&mut e, &TestClock(1)), CommandResult::Handled);
       assert_eq!(e.active().document.buffer.to_string(), "Beta two. Gamma three. Alpha one.\n");
   }
   #[test]
   fn move_sentence_up_swaps_and_caret_travels() {
       let mut e = Editor::new_from_text("Alpha one. Beta two. Gamma three.\n", None, (60, 12));
       crate::derive::rebuild(&mut e);
       // caret in the LAST sentence "Gamma three."
       let at = "Alpha one. Beta two. Gamma three.\n".find("Gamma").unwrap() + 1;
       e.active_mut().document.selection = wordcartel_core::selection::Selection::single(at);
       assert_eq!(move_sentence_up(&mut e, &TestClock(0)), CommandResult::Handled);
       assert_eq!(e.active().document.buffer.to_string(), "Alpha one. Gamma three. Beta two.\n");
       // caret+selection on the MOVED sentence "Gamma three." (now at Beta's old start), head-at-start.
       let p = e.active().document.selection.primary();
       assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "Gamma three.");
       assert_eq!(p.head, p.from());
       // repeat moves the SAME sentence up again.
       assert_eq!(move_sentence_up(&mut e, &TestClock(1)), CommandResult::Handled);
       assert_eq!(e.active().document.buffer.to_string(), "Gamma three. Alpha one. Beta two.\n");
   }
   #[test]
   fn move_sentence_up_stops_at_paragraph_edge() {
       let mut e = Editor::new_from_text("First one. Second two.\n", None, (40, 12));
       crate::derive::rebuild(&mut e);
       e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2); // in "First one."
       assert_eq!(move_sentence_up(&mut e, &TestClock(0)), CommandResult::Noop);
       assert!(e.status.contains("edge"), "edge status: {}", e.status);
       assert_eq!(e.active().document.buffer.to_string(), "First one. Second two.\n"); // unchanged
   }
   #[test]
   fn move_sentence_declines_non_prose() {
       let mut e = Editor::new_from_text("# Heading\n", None, (40, 12));
       crate::derive::rebuild(&mut e);
       e.active_mut().document.selection = wordcartel_core::selection::Selection::single(3);
       assert_eq!(move_sentence_down(&mut e, &TestClock(0)), CommandResult::Noop);
   }
   ```
   Add `struct TestClock(u64); impl wordcartel_core::history::Clock for TestClock { fn now_ms(&self)
   -> u64 { self.0 } }` to the `prose_ops.rs` test module (mirror `textops.rs` tests).
   **Run:** `cargo test -p wordcartel move_sentence` → compile error.

2. **Impl** (`prose_ops.rs`) — one shared core, two thin entry points:
   ```rust
   use crate::nav;
   use wordcartel_core::history::{Clock, EditKind, Transaction};
   use wordcartel_core::selection::Selection;

   /// Direction of a sentence reorder.
   #[derive(Clone, Copy)]
   enum Dir { Up, Down }

   pub(crate) fn move_sentence_up(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
       move_sentence(editor, Dir::Up, clock)
   }
   pub(crate) fn move_sentence_down(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
       move_sentence(editor, Dir::Down, clock)
   }

   /// Swap the caret's sentence A with its neighbour B within the paragraph window, PRESERVING the
   /// exact inter-sentence gap (`{B}{gap}{A}` — the `transpose_words` discipline). Caret+selection land
   /// on the MOVED sentence (head-at-start, F8/C-9). Stop at the paragraph edge (F1). Decline on
   /// non-prose (F3). Gap fate M1: the gap between the pair is preserved verbatim.
   fn move_sentence(editor: &mut Editor, dir: Dir, clock: &dyn Clock) -> CommandResult {
       let h = nav::head(editor);
       // Decline / classify via the shared predicate (SEE==SELECT).
       if super::prose_sentence_at(editor, h).is_err() {
           editor.status = "no sentence here".into();
           return CommandResult::Noop;
       }
       let (ps, pe) = {
           let b = editor.active();
           nav::paragraph_range_at(b.document.blocks(), &b.document.buffer, h)
       };
       let win = editor.active().document.buffer.slice(ps..pe);
       let rel = h.saturating_sub(ps).min(win.len());
       // Window-relative content spans.
       let spans: Vec<(usize, usize)> = wordcartel_core::textobj::sentence_spans(&win).collect();
       if spans.is_empty() { return CommandResult::Noop; }
       // Index of the caret's sentence (attach: caret in the gap → the PRECEDING span, i.e. the last
       // span whose start <= rel; before the first content → span 0).
       let cur = spans.iter().rposition(|&(s, _)| s <= rel).unwrap_or(0);
       let (a_idx, b_idx) = match dir {
           Dir::Down if cur + 1 < spans.len() => (cur, cur + 1),
           Dir::Up   if cur >= 1              => (cur - 1, cur),
           _ => {
               editor.status = "sentence at paragraph edge — break or merge to cross".into();
               return CommandResult::Noop;
           }
       };
       let (a_s, a_e) = spans[a_idx];
       let (b_s, b_e) = spans[b_idx]; // a_idx < b_idx always (ordered)
       let gap = &win[a_e..b_s];
       let out = format!("{}{}{}", &win[b_s..b_e], gap, &win[a_s..a_e]); // {B}{gap}{A}
       let from = ps + a_s;
       let to = ps + b_e;
       // The MOVED sentence is always the caret's (`cur`). In `{B}{gap}{A}` (A=spans[a_idx],
       // B=spans[b_idx]): Down → caret==a_idx (A) lands LAST; Up → caret==b_idx (B) lands FIRST.
       let (moved_from, moved_len) = if a_idx == cur {
           let a_len = a_e - a_s;
           (from + (out.len() - a_len), a_len) // Down: caret sentence lands last
       } else {
           (from, b_e - b_s)                    // Up: caret sentence lands first
       };
       let moved_to = moved_from + moved_len;
       let doc_len = editor.active().document.buffer.len();
       let (cs, edit) = super::build_range_replace(from, to, &out, doc_len);
       // Head-at-start on the moved sentence (C-9): Selection::range(anchor=end, head=start).
       let txn = Transaction::new(cs).with_selection(Selection::range(moved_to, moved_from));
       editor.apply(txn, edit, EditKind::Other, clock);
       let r = super::edit::settle_after_edit(editor);
       editor.status = match dir { Dir::Up => "moved sentence up".into(), Dir::Down => "moved sentence down".into() };
       r
   }
   ```
   **[IMPLEMENTATION NOTE — verify with `cargo test`]** The `moved_from`/`moved_len` offsets and the
   `spans.rposition(|(s,_)| s <= rel)` attach index are confirmed by BOTH the Down
   (`move_sentence_down_swaps_preserving_gap_caret_travels`) and Up
   (`move_sentence_up_swaps_and_caret_travels`) round-trip tests above — do not eyeball; run them.
   Invariant: after the edit the selection covers the caret's (moved) sentence with `head == from`.

3. **Impl — registry rows** (`registry.rs`, Edit menu):
   ```rust
   r.register("move_sentence_up",   "Move Sentence Up",   Some(MenuCategory::Edit), |c| crate::commands::prose_ops::move_sentence_up(c.editor, c.clock));
   r.register("move_sentence_down", "Move Sentence Down", Some(MenuCategory::Edit), |c| crate::commands::prose_ops::move_sentence_down(c.editor, c.clock));
   ```

4. **Green:** `cargo test -p wordcartel move_sentence` → pass (Down + Up round-trip + edge + decline).
   clippy clean.

5. **Commit:** `S4 T5: move_sentence_up/down — gap-preserving reorder, caret travels, stop-at-edge` (+ trailers).

---

### Task 6 — two-region `swap` (loud overlap reject; post-op selection; clears marked_block)

**Deliverable:** exchange the primary `Selection` region with the `MarkedBlock` region as one undo
unit; overlap rejects LOUDLY (never reach `build_multi_replace`'s silent guard); missing either region
→ status; post-op selection holds the moved selection-content head-at-start; `marked_block` cleared.
(Fold preservation is T8.)

**Files:** modify `wordcartel/src/commands/prose_ops.rs` (+ tests); modify `registry.rs` (1 row).

**Interfaces — Produces:** `prose_ops::swap(&mut Editor, &dyn Clock) -> CommandResult`.

**Consumes:** `Editor.active().document.selection`, `Editor.active().marked_block`,
`commands::build_multi_replace`, `super::edit::settle_after_edit`.

**TDD steps.**

1. **Failing tests** (`prose_ops.rs`):
   ```rust
   #[test]
   fn swap_exchanges_selection_and_marked_block() {
       let mut e = Editor::new_from_text("AAAA....BBBB\n", None, (40, 12)); // sel=AAAA(0..4), block=BBBB(8..12)
       crate::derive::rebuild(&mut e);
       e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 4);
       e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 8, end: 12, hidden: false });
       assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Handled);
       assert_eq!(e.active().document.buffer.to_string(), "BBBB....AAAA\n");
       assert!(e.active().marked_block.is_none(), "marked block consumed");
       // selection holds the moved selection-content (AAAA, now at 8..12), head-at-start.
       let p = e.active().document.selection.primary();
       assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "AAAA");
       assert_eq!(p.head, p.from());
   }
   #[test]
   fn swap_rejects_overlap_loudly_without_mutating() {
       let mut e = Editor::new_from_text("abcdefgh\n", None, (40, 12));
       crate::derive::rebuild(&mut e);
       e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5);
       e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: 3, end: 7, hidden: false });
       assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Noop);
       assert!(e.status.contains("overlap"), "status: {}", e.status);
       assert_eq!(e.active().document.buffer.to_string(), "abcdefgh\n", "no mutation on overlap");
   }
   #[test]
   fn swap_requires_both_regions() {
       let mut e = Editor::new_from_text("abc\n", None, (40, 12));
       crate::derive::rebuild(&mut e);
       e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 2);
       // no marked block
       assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Noop);
       assert!(e.status.contains("marked block"));
   }
   ```
   **Run:** `cargo test -p wordcartel swap_` → compile error.

2. **Impl** (`prose_ops.rs`):
   ```rust
   /// `swap` — exchange the primary `Selection` region with the `MarkedBlock` region (F2). ONE undo
   /// unit via `build_multi_replace`. Overlap rejects LOUDLY (never reach the builder's silent
   /// identity-no-op, spec C-2). Gap fate M2: region bytes move verbatim; outside whitespace untouched.
   /// Post-op: selection holds the moved selection-content head-at-start (F8/C-9); marked_block consumed.
   pub(crate) fn swap(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
       let sel = editor.active().document.selection.primary();
       if sel.is_empty() {
           editor.status = "swap needs a selection and a marked block".into();
           return CommandResult::Noop;
       }
       let Some(mb) = editor.active().marked_block else {
           editor.status = "swap needs a selection and a marked block".into();
           return CommandResult::Noop;
       };
       let (s_from, s_to) = (sel.from(), sel.to());
       let (m_from, m_to) = (mb.start, mb.end);
       // Order the two regions; reject overlap (touch-through counts).
       let (r1_from, r1_to, r1_is_sel) = if s_from <= m_from { (s_from, s_to, true) } else { (m_from, m_to, false) };
       let (r2_from, r2_to) = if s_from <= m_from { (m_from, m_to) } else { (s_from, s_to) };
       if r1_to > r2_from {
           editor.status = "can't swap overlapping regions".into();
           return CommandResult::Noop;
       }
       let buf = &editor.active().document.buffer;
       let r1_text = buf.slice(r1_from..r1_to);
       let r2_text = buf.slice(r2_from..r2_to);
       let doc_len = buf.len();
       // ascending, non-overlapping: R1 slot ← R2 text, R2 slot ← R1 text.
       let edits = vec![
           (r1_from, r1_to, r2_text.clone()),
           (r2_from, r2_to, r1_text.clone()),
       ];
       let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
       // Where does the SELECTION's content land? If the selection was R1, its text now sits at R2's
       // slot, shifted by the first replacement's delta (len(R2)-len(R1)); if it was R2, at R1's slot.
       let l1 = r1_to - r1_from;
       let l2 = r2_to - r2_from;
       let (moved_from, moved_len) = if r1_is_sel {
           (r2_from + l2 - l1, l1) // selection was R1 → its text lands at R2 slot (shifted)
       } else {
           (r1_from, l2)           // selection was R2 → its text lands at R1 slot
       };
       let moved_to = moved_from + moved_len;
       let txn = Transaction::new(cs).with_selection(Selection::range(moved_to, moved_from));
       editor.apply(txn, edit, EditKind::Other, clock);
       editor.active_mut().marked_block = None;
       let r = super::edit::settle_after_edit(editor);
       editor.status = "swapped".into();
       r
   }
   ```

3. **Impl — registry row** (`registry.rs`, Block menu, beside `select_marked_block`):
   ```rust
   r.register("swap", "Swap Selection \u{21C4} Block", Some(MenuCategory::Block),
       |c| crate::commands::prose_ops::swap(c.editor, c.clock));
   ```

4. **Green:** `cargo test -p wordcartel swap_` → pass. clippy clean.

5. **Commit:** `S4 T6: two-region swap (loud overlap reject, post-op selection, consumes block)` (+ trailers).

---

### Task 7 — joint edits: `break_paragraph_here` / `merge_paragraph_forward` / `split_sentence_at_caret`

**Deliverable:** the three joint handlers with pinned gap-fate (M3/M4/M5), decline on non-prose,
guards, and F8 head-at-start post-op selection.

**Files:** modify `wordcartel/src/commands/prose_ops.rs` (+ tests); modify `registry.rs` (3 rows).

**Interfaces — Produces:** `prose_ops::break_paragraph_here`, `merge_paragraph_forward`,
`split_sentence_at_caret` (each `(&mut Editor, &dyn Clock) -> CommandResult`).

**Consumes:** `super::prose_sentence_at`, `nav::paragraph_range_at`, `nav::next_paragraph_start`,
`textobj::sentence_spans`, `super::build_range_replace`.

**TDD steps.**

1. **Failing tests** (`prose_ops.rs`) — one per handler, asserting gap-fate:
   ```rust
   #[test]
   fn break_paragraph_here_promotes_sentence() {
       let mut e = Editor::new_from_text("Alpha one. Beta two.\n", None, (40, 12));
       crate::derive::rebuild(&mut e);
       let at = "Alpha one. Beta two.\n".find("Beta").unwrap();
       e.active_mut().document.selection = wordcartel_core::selection::Selection::single(at);
       assert_eq!(break_paragraph_here(&mut e, &TestClock(0)), CommandResult::Handled);
       assert_eq!(e.active().document.buffer.to_string(), "Alpha one.\n\nBeta two.\n");
       let p = e.active().document.selection.primary();
       assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "Beta two.");
       assert_eq!(p.head, p.from());
   }
   #[test]
   fn merge_paragraph_forward_single_spaces() {
       let mut e = Editor::new_from_text("Para one.\n\nPara two.\n", None, (40, 12));
       crate::derive::rebuild(&mut e);
       e.active_mut().document.selection = wordcartel_core::selection::Selection::single(2); // in Para one
       assert_eq!(merge_paragraph_forward(&mut e, &TestClock(0)), CommandResult::Handled);
       assert_eq!(e.active().document.buffer.to_string(), "Para one. Para two.\n");
       // F8: the absorbed paragraph's first sentence is selected, head-at-start.
       let p = e.active().document.selection.primary();
       assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "Para two.");
       assert_eq!(p.head, p.from(), "F8: caret head-at-start on the absorbed sentence");
   }
   #[test]
   fn split_sentence_at_caret_inserts_terminator_and_capitalizes() {
       let mut e = Editor::new_from_text("the cat sat on the mat\n", None, (40, 12));
       crate::derive::rebuild(&mut e);
       let at = "the cat sat on the mat\n".find(" on").unwrap(); // caret before " on" (a space)
       e.active_mut().document.selection = wordcartel_core::selection::Selection::single(at);
       assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Handled);
       assert_eq!(e.active().document.buffer.to_string(), "the cat sat. On the mat\n");
       // F8: the SECOND sentence is selected, caret head-at-start on the capitalized 'O' (NOT the
       // retained leading space — Codex finding 3).
       let p = e.active().document.selection.primary();
       assert_eq!(e.active().document.buffer.slice(p.from()..p.to()), "On the mat");
       assert_eq!(p.head, p.from());
       assert_eq!(e.active().document.buffer.slice(p.head..p.head + 1), "O", "caret on the capital, not the space");
   }
   #[test]
   fn split_rejects_gap_and_edge() {
       let mut e = Editor::new_from_text("One two. Three four.\n", None, (40, 12));
       crate::derive::rebuild(&mut e);
       let gap = "One two. Three four.\n".find(" Three").unwrap(); // in the inter-sentence gap: head > st
       e.active_mut().document.selection = wordcartel_core::selection::Selection::single(gap);
       assert_eq!(split_sentence_at_caret(&mut e, &TestClock(0)), CommandResult::Noop);
   }
   ```
   **Run:** `cargo test -p wordcartel break_paragraph_here merge_paragraph_forward split_sentence` →
   compile error.

2. **Impl** (`prose_ops.rs`):
   ```rust
   /// `break_paragraph_here` — the caret's sentence (and all after it in the paragraph) becomes a new
   /// paragraph. Gap fate M3: consume the single separator before the sentence, insert "\n\n". Decline
   /// on non-prose; Noop if already at a paragraph start. F8: the promoted sentence is selected.
   pub(crate) fn break_paragraph_here(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
       let h = nav::head(editor);
       let (sf, st) = match super::prose_sentence_at(editor, h) {
           Ok(s) => s, Err(_) => { editor.status = "no sentence here".into(); return CommandResult::Noop; }
       };
       let (ps, _pe) = {
           let b = editor.active();
           nav::paragraph_range_at(b.document.blocks(), &b.document.buffer, h)
       };
       if sf <= ps { editor.status = "already at a paragraph start".into(); return CommandResult::Noop; }
       // Consume the whitespace run immediately before the sentence content.
       let buf = &editor.active().document.buffer;
       let head_text = buf.slice(ps..sf);
       let trimmed = head_text.trim_end_matches(char::is_whitespace).len();
       let gap_start = ps + trimmed;
       let doc_len = buf.len();
       let (cs, edit) = super::build_range_replace(gap_start, sf, "\n\n", doc_len);
       // Sentence shifts by delta = 2 - (sf - gap_start).
       let delta = 2isize - (sf - gap_start) as isize;
       let new_sf = (sf as isize + delta) as usize;
       let new_st = (st as isize + delta) as usize;
       let txn = Transaction::new(cs).with_selection(Selection::range(new_st, new_sf));
       editor.apply(txn, edit, EditKind::Other, clock);
       let r = super::edit::settle_after_edit(editor);
       editor.status = "split paragraph".into();
       r
   }

   /// `merge_paragraph_forward` — join the caret's paragraph with the next. Gap fate M4: replace the
   /// paragraph separator with ONE space. Decline on non-prose; Noop if no next paragraph or the next
   /// block is non-prose. F8: the absorbed paragraph's first sentence is selected.
   pub(crate) fn merge_paragraph_forward(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
       let h = nav::head(editor);
       if super::prose_sentence_at(editor, h).is_err() {
           editor.status = "no paragraph here".into(); return CommandResult::Noop;
       }
       let (_ps, pe) = {
           let b = editor.active();
           nav::paragraph_range_at(b.document.blocks(), &b.document.buffer, h)
       };
       let (nps, next_is_prose) = {
           let b = editor.active();
           let nps = nav::next_paragraph_start(b.document.blocks(), &b.document.buffer, pe);
           let prose = nps < b.document.buffer.len()
               && crate::ventilate::line_content_byte(&b.document.buffer, b.document.buffer.byte_to_line(nps))
                   .map(|c| b.document.blocks().role_at(c) == wordcartel_core::style::BlockRole::Paragraph)
                   .unwrap_or(false);
           (nps, prose)
       };
       if nps >= editor.active().document.buffer.len() {
           editor.status = "no paragraph to merge".into(); return CommandResult::Noop;
       }
       if !next_is_prose {
           editor.status = "can't merge across a non-paragraph block".into(); return CommandResult::Noop;
       }
       // The absorbed paragraph's first sentence begins at `nps` (its content start) → after merge it
       // sits at `pe + 1` (one space replaces [pe, nps)). Select it head-at-start.
       let doc_len = editor.active().document.buffer.len();
       let (cs, edit) = super::build_range_replace(pe, nps, " ", doc_len);
       let new_start = pe + 1;
       // Length of the absorbed first sentence: recompute from the pre-edit next paragraph window.
       let sent_len = {
           let b = editor.active();
           let (n_ps, n_pe) = nav::paragraph_range_at(b.document.blocks(), &b.document.buffer, nps);
           let nwin = b.document.buffer.slice(n_ps..n_pe);
           wordcartel_core::textobj::sentence_spans(&nwin).next().map(|(s, e2)| e2 - s).unwrap_or(0)
       };
       let txn = Transaction::new(cs).with_selection(Selection::range(new_start + sent_len, new_start));
       editor.apply(txn, edit, EditKind::Other, clock);
       let r = super::edit::settle_after_edit(editor);
       editor.status = "merged paragraph".into();
       r
   }

   /// `split_sentence_at_caret` — turn one sentence into two at the caret. Gap fate M5: insert ". "
   /// (or "." if the next char is whitespace — no double space) and uppercase the next word's initial.
   /// Interior guard (finding 3): `sf < head < st` (a gap caret has head > st and is rejected).
   pub(crate) fn split_sentence_at_caret(editor: &mut Editor, clock: &dyn Clock) -> CommandResult {
       let h = nav::head(editor);
       let (sf, st) = match super::prose_sentence_at(editor, h) {
           Ok(s) => s, Err(_) => { editor.status = "no sentence here".into(); return CommandResult::Noop; }
       };
       if !(sf < h && h < st) {
           editor.status = "place the caret inside a sentence to split".into();
           return CommandResult::Noop;
       }
       let buf = &editor.active().document.buffer;
       let after = buf.slice(h..st);
       let next_is_ws = after.chars().next().is_some_and(char::is_whitespace);
       let ins = if next_is_ws { ".".to_string() } else { ". ".to_string() };
       // The next word's initial (first alphabetic at/after the caret) — the SECOND sentence's content
       // start. Capitalize it only when it is lowercase (never re-case a proper noun already capital).
       let word = after.char_indices().find(|&(_, c)| c.is_alphabetic());
       let doc_len = buf.len();
       let (edits, case_delta): (Vec<(usize, usize, String)>, isize) = match word {
           Some((off, ch)) if ch.is_lowercase() => {
               let ci = h + off;
               let upper: String = ch.to_uppercase().collect();
               let delta = upper.len() as isize - ch.len_utf8() as isize;
               // Ascending, non-overlapping (touching at h when off==0 is allowed): terminator then case-map.
               (vec![(h, h, ins.clone()), (ci, ci + ch.len_utf8(), upper)], delta)
           }
           _ => (vec![(h, h, ins.clone())], 0), // uppercase initial or no following word → terminator only
       };
       let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
       // F8: the second sentence begins at the next word's initial, shifted by the inserted terminator
       // (NOT `h + ins.len()`, which would include the retained leading space — Codex finding 3). No
       // following word → just after the terminator.
       let new_second_from = match word { Some((off, _)) => h + off + ins.len(), None => h + ins.len() };
       let new_st = (st as isize + ins.len() as isize + case_delta) as usize;
       let txn = Transaction::new(cs).with_selection(Selection::range(new_st, new_second_from));
       editor.apply(txn, edit, EditKind::Other, clock);
       let r = super::edit::settle_after_edit(editor);
       editor.status = "split sentence".into();
       r
   }
   ```
   **[IMPLEMENTATION NOTE — resolved sub-decisions]** (a) The capitalize offset arithmetic is delicate
   — verify `new_second_from = h + off + ins.len()` (the next word's initial, NOT `h + ins.len()`) and
   `new_st` against `split_sentence_at_caret_inserts_terminator_and_capitalizes`'s F8 asserts with
   `cargo test`, not by inspection; invariant: the selection covers the new SECOND sentence with
   `head == from` landing on the capital. (b) When the next initial is non-lowercase (proper noun
   already capitalized, or no following word), split inserts the terminator only (no case change) —
   the honest behavior (do not down/re-case a proper noun). (c) `break_paragraph_here`'s status string
   is `"split paragraph"` per spec §5.1; `merge_paragraph_forward`'s absorbed-sentence selection offset
   (`new_start = pe + 1`, `sent_len` from the pre-edit next-paragraph window) is likewise pinned by the
   merge test's F8 asserts.

3. **Impl — registry rows** (`registry.rs`, Edit menu):
   ```rust
   r.register("break_paragraph_here",    "Break Paragraph Here",    Some(MenuCategory::Edit), |c| crate::commands::prose_ops::break_paragraph_here(c.editor, c.clock));
   r.register("merge_paragraph_forward", "Merge Paragraph Forward", Some(MenuCategory::Edit), |c| crate::commands::prose_ops::merge_paragraph_forward(c.editor, c.clock));
   r.register("split_sentence_at_caret", "Split Sentence",          Some(MenuCategory::Edit), |c| crate::commands::prose_ops::split_sentence_at_caret(c.editor, c.clock));
   ```

4. **Green:** `cargo test -p wordcartel break_paragraph_here merge_paragraph_forward split_sentence` →
   pass. clippy clean. Each edit is one undo unit — add a quick `e.undo()` byte-identical assert.

5. **Commit:** `S4 T7: joint edits — break/merge paragraph + split sentence with pinned gap fate` (+ trailers).

---

### Task 8 — fold survival across move/swap (C-7/C-12/C-13) wired into `swap` + `block_move`

**Deliverable:** the shared `fold::corrected_after_move` helper and its two-rebuild bracket wired into
BOTH the new `swap` (T6) and the existing `blocks_marked::block_move`, so a moved/swapped folded
section STAYS FOLDED exactly once, the view reflects it, and a two-region swap cannot self-clobber.

**Files:**
- modify `wordcartel/src/fold.rs` (`corrected_after_move` + test)
- modify `wordcartel/src/blocks_marked.rs` (`block_move` bracket)
- modify `wordcartel/src/commands/prose_ops.rs` (`swap` bracket)
- tests in `fold.rs`, `blocks_marked.rs`, `prose_ops.rs`

**Interfaces — Produces:**
- `fold::corrected_after_move(folds: &FoldState, regions: &[(usize, usize, usize)], cs:
  &wordcartel_core::change::ChangeSet) -> std::collections::BTreeSet<usize>` — `regions` are
  `(from, to, dest)` per relocated span.

**Consumes:** `FoldState::{folded, replace_folded}`, `change::map_pos` (After bias — Critical 2),
`derive::rebuild`.

**Design (spec §7.3 — replace_folded variant, one-set, C-12 no-interleave; two rebuilds C-13):**
compute the corrected set from the PRE-edit folded set — moved anchors go to `dest + (a - from)`
(geometry), stationary anchors go to `map_pos(a, cs)` (After bias — a stationary heading at the
destination caret advances past the inserted block; `map_pos_before` would leave it stranded, Critical 2).
Sequence: capture pre + compute corrected → `apply` (rebuild #1 settles the tree) → `replace_folded`
(override apply's remap with the corrected set) → `rebuild` #2 (reconcile validates against
heading_starts + relayout). Skip entirely when `folds.is_empty()`.

**TDD steps.**

1. **Failing test — the pure helper** (`fold.rs` tests):
   ```rust
   #[test]
   fn corrected_after_move_relocates_moved_and_remaps_stationary() {
       use wordcartel_core::change::{ChangeSet, Op, Tendril};
       // Pre-edit folds at 0 (moved region [0,4)→dest 10) and 20 (stationary).
       let mut fs = FoldState::default();
       fs.toggle(0); fs.toggle(20);
       // Insert 2 bytes before the stationary anchor 20 (at 18) → After-bias map_pos shifts it to 22.
       let cs = ChangeSet::from_ops(vec![Op::Retain(18), Op::Insert(Tendril::from("XY")), Op::Retain(12)], 30);
       let corrected = corrected_after_move(&fs, &[(0, 4, 10)], &cs);
       assert!(corrected.contains(&10), "moved anchor → dest+rel (geometry)");
       assert!(corrected.contains(&22), "stationary anchor → map_pos (After bias)");
       assert!(!corrected.contains(&0));
   }
   #[test]
   fn corrected_after_move_stationary_at_destination_caret_advances_past_block() {
       use wordcartel_core::change::{ChangeSet, Op, Tendril};
       // Critical-2 case: block [20,24) moved to caret=10 (caret < b.start). A STATIONARY folded
       // heading sits EXACTLY at 10 (the destination). It must advance to 14 (past the inserted block),
       // NOT stay at 10 where the moved heading lands — else the two folds collide.
       let mut fs = FoldState::default();
       fs.toggle(20); fs.toggle(10);
       // build_multi_replace([(10,10,"ABCD"),(20,24,"")]) shape: Retain(10) Insert(4) Retain(10) Delete(4) Retain(2).
       let cs = ChangeSet::from_ops(
           vec![Op::Retain(10), Op::Insert(Tendril::from("ABCD")), Op::Retain(10),
                Op::Delete(4), Op::Retain(2)], 26);
       let corrected = corrected_after_move(&fs, &[(20, 24, 10)], &cs);
       assert!(corrected.contains(&10), "moved heading → dest 10 (geometry)");
       assert!(corrected.contains(&14), "stationary heading at caret 10 → 14 (past the inserted block, After bias)");
       assert!(!corrected.contains(&20));
       assert_eq!(corrected.len(), 2, "two distinct folds — no collision (map_pos_before would give both at 10)");
   }
   ```
   **Run:** `cargo test -p wordcartel corrected_after_move` → compile error, then both pass.

2. **Impl — the helper** (`fold.rs`):
   ```rust
   /// The corrected fold-anchor set after a region relocation (spec §7.3, C-7/C-12). `regions` are
   /// `(from, to, dest)` per relocated span. A folded anchor INSIDE a region moves by geometry to
   /// `dest + (anchor - from)`; a stationary anchor is remapped through the edit with **After bias
   /// (`change::map_pos`)** — a relocated block inserted AT a stationary heading's byte must push that
   /// heading PAST the inserted block, not leave it stranded at the block's start. (Before-bias
   /// `map_pos_before` — what `Buffer::apply` uses for ordinary typing at a heading — returns the
   /// stationary anchor UNCHANGED for an insert exactly at it (change.rs, `pos == old && !prev_was_
   /// delete`), so a stationary heading at the destination caret would clobber the moved heading's
   /// destination fold or fold the wrong heading. This helper OVERRIDES apply's remap via
   /// `replace_folded`, so using After bias here is correct and intentional.) Computed as ONE set so
   /// there is no per-region interleaving that could self-clobber a shared collapse/destination byte.
   pub fn corrected_after_move(
       folds: &FoldState,
       regions: &[(usize, usize, usize)],
       cs: &wordcartel_core::change::ChangeSet,
   ) -> std::collections::BTreeSet<usize> {
       let mut out = std::collections::BTreeSet::new();
       for &a in folds.folded() {
           match regions.iter().find(|(from, to, _)| a >= *from && a < *to) {
               Some(&(from, _to, dest)) => { out.insert(dest + (a - from)); }
               None => { out.insert(wordcartel_core::change::map_pos(a, cs)); }
           }
       }
       out
   }
   ```

3. **Impl — wire into `block_move`** (`blocks_marked.rs`). `block_move` currently builds `(cs, edit)`
   then calls `apply_edit(editor, cs, edit, new_caret, clock)`. Replace that tail:
   ```rust
   let (cs, edit) = crate::commands::build_multi_replace(&edits, doc_len);
   // dest of the moved block's start in FINAL coords: caret (moved before) or caret-len (moved after).
   let dest = if caret < b.start { caret } else { caret - (b.end - b.start) };
   let corrected = if !editor.active().folds.is_empty() {
       Some(crate::fold::corrected_after_move(&editor.active().folds, &[(b.start, b.end, dest)], &cs))
   } else { None };
   apply_edit(editor, cs, edit, new_caret, clock); // apply + rebuild #1 (settle)
   if let Some(c) = corrected {
       editor.active_mut().folds.replace_folded(c);
       crate::derive::rebuild(editor);             // rebuild #2 (relayout + reconcile)
   }
   editor.active_mut().marked_block = None;
   editor.status = "block moved".into();
   ```

4. **Impl — wire into `swap`** (`prose_ops.rs`). **C-13 two-rebuild bracket (Critical 1).** Grounded:
   `Editor::apply` (editor.rs) delegates to `Buffer::apply` and does NOT rebuild — the ONLY rebuild in
   the current `swap` tail is `settle_after_edit` (commands/edit.rs, which calls `derive::rebuild`).
   So `swap` must add an EXPLICIT settling rebuild between `apply` and `replace_folded`, and let
   `settle_after_edit` provide the relayout rebuild. Replace T6's `swap` tail (from `editor.apply`
   onward) with:
   ```rust
   // (after building `edits` and `let (cs, edit) = build_multi_replace(&edits, doc_len);` and l1/l2 —
   //  compute `corrected` from `&cs` BEFORE `Transaction::new(cs)` moves cs.)
   let regions = [
       (r1_from, r1_to, r2_from + l2 - l1), // R1's content → R2 slot (shifted by len delta)
       (r2_from, r2_to, r1_from),           // R2's content → R1 slot
   ];
   let corrected = if !editor.active().folds.is_empty() {
       Some(crate::fold::corrected_after_move(&editor.active().folds, &regions, &cs))
   } else { None };
   let txn = Transaction::new(cs).with_selection(Selection::range(moved_to, moved_from)); // moves cs
   editor.apply(txn, edit, EditKind::Other, clock); // Buffer::apply only — NO rebuild
   editor.active_mut().marked_block = None;
   if let Some(c) = corrected {
       crate::derive::rebuild(editor);              // REBUILD #1 — settle the tree (heading_starts valid)
       editor.active_mut().folds.replace_folded(c); // override apply's remap with the corrected set
   }
   let r = super::edit::settle_after_edit(editor);  // REBUILD #2 — relayout + reconcile the corrected folds
   editor.status = "swapped".into();
   r
   ```
   (This REPLACES T6's `let txn = …; editor.apply(…); editor.active_mut().marked_block = None; let r =
   settle_after_edit(…);` tail — the `corrected`/`txn` order matters: `corrected_after_move` borrows
   `&cs`, so it must run before `Transaction::new(cs)` consumes `cs`.)
   > **Two rebuilds (C-13), grounded:** `apply` (no rebuild) → `derive::rebuild` (settle) →
   > `replace_folded` → `settle_after_edit`'s `derive::rebuild` (relayout). The settling rebuild is
   > required by the spec even though the pre-edit-geometry `corrected` set does not itself read
   > post-settle `heading_starts` — C-13 is committed law; do not collapse it. When there are no folds,
   > the settling rebuild is skipped and only `settle_after_edit` runs (one rebuild — the fold path is
   > the only two-rebuild path, and it is cold). `corrected` is computed from the PRE-edit folds + `cs`,
   > independent of apply's own remap.

5. **Failing/green tests** (`prose_ops.rs`, `blocks_marked.rs`) — assert the SPECIFIC relocated heading
   byte (a stale fold at the wrong heading would pass a bare `len == 1` — finding 6):
   ```rust
   #[test]
   fn swap_keeps_a_folded_section_folded_at_its_new_byte() {
       // A=[0,b) B=[b,len). Fold A; select A; mark B; swap → buffer is B_text ++ A_text, so A's heading
       // relocates to `len - (b - 0)` = len - b (l1 = b, A lands at r1_from + l2 = 0 + (len - b)).
       let doc = "## A\n\nbody a.\n\n## B\n\nbody b.\n";
       let mut e = Editor::new_from_text(doc, None, (60, 20));
       crate::derive::rebuild(&mut e);
       let a = doc.find("## A").unwrap(); // 0
       let b = doc.find("## B").unwrap();
       let len = doc.len();
       e.active_mut().folds.toggle(a);
       let (a_from, a_to) = crate::commands::section_range_at(&e, a + 1).unwrap();
       let (b_from, b_to) = crate::commands::section_range_at(&e, b + 1).unwrap();
       e.active_mut().document.selection = wordcartel_core::selection::Selection::range(a_from, a_to);
       e.active_mut().marked_block = Some(crate::editor::MarkedBlock { start: b_from, end: b_to, hidden: false });
       assert_eq!(swap(&mut e, &TestClock(0)), CommandResult::Handled);
       let a_new = len - b; // A's heading destination (byte-length-preserving swap)
       let folded = e.active().folds.folded();
       assert!(folded.contains(&a_new), "A's heading is folded at its NEW byte {a_new}: {folded:?}");
       assert!(!folded.contains(&0), "the fold did NOT stay on B's heading (now at 0)");
       assert_eq!(folded.len(), 1, "exactly one fold — no double, no drop");
   }
   ```
   And an equivalent `block_move_keeps_a_folded_section_folded_at_its_new_byte` in `blocks_marked.rs`
   tests: fold a section, mark it, move the caret to a destination BEFORE it, `block_move`, and assert
   the section's heading is folded at its computed destination byte (`dest = caret`) and `folded().len()
   == 1`. (The stationary-heading-at-destination arithmetic is proven by the
   `corrected_after_move_stationary_at_destination_caret_advances_past_block` unit test above.)

6. **Green:** `cargo test -p wordcartel corrected_after_move
   swap_keeps_a_folded_section_folded_at_its_new_byte
   block_move_keeps_a_folded_section_folded_at_its_new_byte` (incl. the stationary-at-caret unit test)
   + the T6/T7 tests still green. clippy clean.

7. **Commit:** `S4 T8: fold survival across move/swap (corrected_after_move, two-rebuild, no double-fold)`
   (+ trailers).

---

### Task 9 — B10: EOF caret clamp fix in `nav::caret_line`

**Deliverable:** a caret at `buf.len()` maps to the trailing (phantom) line's row, not the last content
line; non-EOF placement unchanged; verified on AND off the lens (shared clamp).

**Files:** modify `wordcartel/src/nav.rs` (`caret_line`) + tests in `nav.rs`.

**Interfaces — Produces:** `nav::caret_line` (same signature, corrected behavior).

**TDD steps.**

1. **Failing test** (`nav.rs` tests):
   ```rust
   #[test]
   fn caret_line_at_eof_maps_to_phantom_line_not_last_content() {
       // "a\nb\n" has content lines 0,1 and a trailing phantom line 2 (after the final \n).
       let mut e = Editor::new_from_text("a\nb\n", None, (20, 8));
       crate::derive::rebuild(&mut e);
       let len = e.active().document.buffer.len();
       e.active_mut().document.selection = wordcartel_core::selection::Selection::single(len);
       assert_eq!(caret_line(&e), 2, "EOF caret sits on the trailing phantom line (B10)");
       // non-EOF unchanged
       e.active_mut().document.selection = wordcartel_core::selection::Selection::single(0);
       assert_eq!(caret_line(&e), 0);
   }
   ```
   **Run:** `cargo test -p wordcartel caret_line_at_eof_maps_to_phantom_line_not_last_content` → fails
   (current clamp returns 1).

2. **Impl** (`nav.rs::caret_line`) — current:
   ```rust
   pub fn caret_line(editor: &Editor) -> usize {
       let buf = &editor.active().document.buffer;
       let h = head(editor);
       if buf.is_empty() { return 0; }
       buf.byte_to_line(h.min(buf.len().saturating_sub(1)))
   }
   ```
   B10 fix — drop the `len-1` clamp; `byte_to_line` is defined for `h == len` (the phantom line):
   ```rust
   pub fn caret_line(editor: &Editor) -> usize {
       let buf = &editor.active().document.buffer;
       let h = head(editor);
       if buf.is_empty() { return 0; }
       // B10: do NOT clamp to len-1 — an EOF caret (h == len) belongs on the trailing phantom line,
       // not glued to the last content line. `byte_to_line` accepts h in 0..=len.
       buf.byte_to_line(h.min(buf.len()))
   }
   ```
   > Verify `TextBuffer::byte_to_line` accepts `b == len` (grep its body — it delegates to ropey's
   > `byte_to_line`, which is defined for `0..=len_bytes`). If a downstream consumer of `caret_line`
   > assumed the old clamp (grep `caret_line(` callers), run the full `nav`/`render` suites — the fix
   > must not regress non-EOF placement (the test's second assert guards this).

3. **Green:** `cargo test -p wordcartel caret_line_at_eof_maps_to_phantom_line_not_last_content` +
   the full `-p wordcartel nav` and `render` suites → pass (on and off lens — `caret_line` is the
   shared path, so both are covered by the same fix; add an on-lens assert if a lens-specific caret
   test exists). clippy clean.

4. **Commit:** `S4 T9: B10 — EOF caret maps to the trailing phantom line` (+ trailers).

---

### Task 10 — e2e journey + budgets + palette completeness

**Deliverable:** an in-process `reduce → advance → render` journey (`e2e.rs`) exercising lens-on →
select-sentence → move-sentence-up → undo byte-identical → lens re-derives cleanly; plus the
`TestBackend` paint assertions for SEE==SELECT multi-row + folded-section, and the budget/contract
gate re-checks.

**Files:** modify `wordcartel/src/e2e.rs` (test) + `render.rs` tests (paint asserts).

**Interfaces — Consumes:** the shipped T1–T9 commands via the registry/reduce path.

**TDD steps.**

1. **Failing/green e2e** (`e2e.rs` — using the REAL harness primitives: `Harness::new(text, None,
   size)`, `h.editor.borrow_mut()`, `h.alt('v')` (toggle_ventilate keybinding), `SharedClock::new(0)`,
   `commands::run` for run-routed commands / a direct leaf call for `prose_ops`, `ed.active().document
   .buffer.slice(..)` for the buffer):
   ```rust
   #[test]
   fn e2e_s4_lens_move_sentence_then_undo_is_byte_identical() {
       let doc = "Alpha one. Beta two. Gamma three.\n";
       let mut h = Harness::new(doc, None, (60, 12));
       h.alt('v'); // lens ON (toggle_ventilate)
       { let mut ed = h.editor.borrow_mut();
         ed.active_mut().document.selection = wordcartel_core::selection::Selection::single(2); } // in "Alpha one."
       { let mut ed = h.editor.borrow_mut();
         crate::commands::prose_ops::move_sentence_down(&mut ed, &SharedClock::new(0)); }
       { let ed = h.editor.borrow();
         let len = ed.active().document.buffer.len();
         assert_eq!(ed.active().document.buffer.slice(0..len), "Beta two. Alpha one. Gamma three.\n"); }
       { let mut ed = h.editor.borrow_mut(); ed.undo(); }
       { let ed = h.editor.borrow();
         let len = ed.active().document.buffer.len();
         assert_eq!(ed.active().document.buffer.slice(0..len), doc, "undo restores byte-identical"); }
       h.render(); // lens re-derives without panic/blank
   }
   ```
   (`SharedClock::new` and `Harness`/`.alt(..)`/`.render()` are the real `e2e.rs` symbols — verified;
   `prose_ops::move_sentence_down` is called directly because it is a leaf handler, not routed through
   `commands::run`.)

2. **Paint asserts** (`render.rs` tests, `TestBackend`):
   - **SEE==SELECT multi-row (spec §8 probe 1):** lens ON, `select_sentence` a sentence wrapping to
     multiple ventilated rows; assert an `SE::Selection` highlight on every row of the group (reuse the
     existing `row_has_highlight(buf, row)` helper — grep it in `render.rs` tests).
   - **Folded-section paint (probe 2):** `select_section` a folded section; assert the folded heading
     row carries the selection highlight and hidden rows are not drawn (`active_fold_view().is_hidden`).

3. **Gate re-checks:** `cargo test -p wordcartel --test module_budgets` (no hub grew — new logic is in
   `prose_ops.rs` + data rows); palette-completeness test green (all 8 commands listed);
   `cargo clippy --workspace --all-targets` clean; `cargo test --workspace` green. Run
   `scripts/smoke/run.sh` and record the one-line summary.

4. **Commit:** `S4 T10: e2e lens+surgery journey + SEE==SELECT/fold paint asserts + gate re-checks`
   (+ trailers).

---

## Self-review (writing-plans checklist)

- **Spec coverage:** F1 (T5), F2 (T6 + two-region law in constraints), F3 decline (T1, applied in
  T5/T7), F4 ladder+stateless+sel_history census (T3), F5 SP-7+segment+count_region (T4), F6
  break/merge/split + gap-fate (T7), F7 select_section + fold survival (T2 + T8), F8 head-at-start
  (T2 wiring, applied T5–T7); C-7/C-12/C-13 (T8); C-9 (T2); C-11 content-byte (T1); B10 (T9);
  SEE==SELECT single-source + probes (T1, T10). Every spec deliverable maps to a task.
- **No placeholders:** every step has complete, grounded Rust. Two spots carry explicit
  **IMPLEMENTATION NOTE**s where offset arithmetic must be `cargo`-verified (move_sentence per-direction
  `moved_from`; split capitalize offsets) — these are flagged as verify-with-test, not left vague.
- **Type consistency:** handler signatures `(&mut Editor, &dyn Clock) -> CommandResult` (count_region
  `(&mut Editor)`); `Selection::range(anchor, head)` used head-at-start everywhere (C-9);
  `build_range_replace`/`build_multi_replace` return `(ChangeSet, Edit)`; `region_stats(&str) ->
  RegionStats`; `corrected_after_move(&FoldState, &[(usize,usize,usize)], &ChangeSet) -> BTreeSet`.
- **Grounding re-anchored by name:** `settle_after_edit` (`pub(super)` in `commands/edit.rs`),
  `editor.apply` (Editor method, not Buffer's), `register`/`register_stateful` (private, in `builtins`),
  `run(c, Command)` helper, `MenuCategory::{Edit,Block,View}`, `change::{map_pos, map_pos_before}` (pub;
  After- vs Before-bias), `FoldState`
  API, `derive::{line_start,rebuild}` (re-exported), `ventilate::{line_content_byte, prose_block_at}`,
  `nav::{head, paragraph_range_at, next_paragraph_start, caret_line}`. Line numbers omitted where they
  would drift; the implementer locates by name.
- **Anti-regrowth:** all handlers in `commands/prose_ops.rs`; the ladder is `const LADDER`; commands
  are registry rows; no `reduce`/`place_cursor`/`run`-arm growth. `too_many_lines`: each handler is
  under 100 lines; `move_sentence`/`split_sentence_at_caret` are the longest — if either trips the
  gate, split a helper (do NOT add a blanket allow).

## Open implementation choices resolved by this plan (spec left the mechanism to the plan)

1. **Fold correction shape:** the spec blessed both "remove/toggle two-phase" and "replace_folded
   wholesale". **Chosen: `replace_folded` wholesale** (T8) — compute the corrected set once from the
   pre-edit folds (moved→geometry, stationary→`map_pos` After-bias, Critical 2), override apply's remap.
   It is inherently C-12 order-independent (one set, no interleaving) and needs no post-settle
   `heading_starts` read (destinations are heading bytes by construction; the rebuild's `reconcile_to`
   validates). **Both paths carry the C-13 two-rebuild bracket (Critical 1):** `block_move` = `apply_edit`
   (apply + rebuild #1 settle) → `replace_folded` → `derive::rebuild` (#2 relayout); `swap` = `apply`
   (no rebuild) → `derive::rebuild` (#1 settle) → `replace_folded` → `settle_after_edit` (#2 relayout).
   The no-folds case skips the settling rebuild (one rebuild, the common path).
2. **`prose_sentence_at` home:** `commands.rs` (`pub` so `prose_ops` reaches it via `super::`) — one
   predicate for `select_sentence`, the `Scope::Sentence` arm, and all mutations.
3. **Decline routing:** `SelectScope` arm special-cases `Sentence`/`Section` (Result/Option decline);
   `Word`/`Paragraph`/`Document` stay total via `scope_range_at`.
4. **`RegionStats` API:** free fn `region_stats(&str) -> RegionStats` in `wordcartel-core::count` (no
   `::of` ctor) — the one SP-7 helper; the gutter keeps its per-span `count::word_count` call (spec §6.1),
   which `region_stats` composes, so there is a single word-counting implementation.
5. **split next-word capitalization offset (Codex-r4 #3):** the second sentence's F8 selection begins at
   the next word's initial (`h + off + ins.len()`), not just past the terminator — so the caret lands on
   the capital, not the retained leading space. When the next initial is already uppercase or absent,
   split inserts the terminator only (no re-casing a proper noun).
