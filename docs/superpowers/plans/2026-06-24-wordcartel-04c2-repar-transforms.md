# Wordcartel 4c-2 — `repar` In-Process Transforms Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Reflow / Unwrap / Ventilate as in-process, markdown-aware editor commands that reformat the selection (snapped to whole block-tree blocks) or whole buffer as one undoable edit, invoked via `Ctrl+T` → a single-key modal chooser, synchronous under 1 MiB with an async version-discard fallback above.

**Architecture:** A pure typed wrapper (`run_transform`) is the only code that touches the `repar` library's stringly public API. Region selection snaps to the buffer's already-maintained block tree (Effort 3). A `Ctrl+T` modal chooser (reusing the existing keypress-modal `Prompt` infra) dispatches the transform; small regions run inline, large regions go through a dedicated thread → `Msg::TransformDone` → a version-discard merge that mirrors 4c-1's `FilterDone` exactly. All new dispatch/threading lives in the shell crate; `wordcartel-core` is untouched.

**Tech Stack:** Rust; `repar` v0.9.10 path dependency (`../../par-command/repar`, I/O-free, `#![forbid(unsafe_code)]`); existing `wordcartel` shell crate (`commands::build_range_replace`, `Buffer::apply`/`EditKind::Other`, the modal `Prompt`/`PromptAction`, `wordcartel_core::block_tree`, the `Msg`/`reduce` loop, `registry`/`input` keymap).

**Spec:** `docs/superpowers/specs/2026-06-24-wordcartel-04c2-repar-transforms-design.md` (Codex-reviewed).

## Global Constraints

- `#![forbid(unsafe_code)]` holds in the shell crate; `wordcartel-core` stays IO/thread-free (the transform's threading + dispatch live in the shell crate `transform.rs`/`app.rs`, never in core).
- `repar = { path = "../../par-command/repar" }` is added to **`wordcartel/Cargo.toml` only** (NOT `wordcartel-core`). Path is relative to that manifest; verify it resolves at implementation time.
- **Filter is text-only and untouched:** do NOT modify 4c-1's `filter.rs`/`dispatch_filter`/`FilterDone`. Transforms are a SEPARATE in-process path; there is no subprocess and no `repar` binary spawn.
- **Region unit is the block tree, NOT blank lines** (Codex CRITICAL): a selection snaps outward to the whole top-level `Block`s it intersects, so fenced code / lists / tables / blockquotes are never split.
- **Width default = 72** (`DEFAULT_REFLOW_WIDTH: u32 = 72`); **async threshold = 1 MiB** (`TRANSFORM_ASYNC_THRESHOLD: usize = 1 << 20`).
- **One undoable edit** per transform (`EditKind::Other`); output identical to input → **no edit** committed.
- `Ctrl+T` opens the chooser **only in normal mode**; while a prompt/minibuffer is open the keypress is consumed by that modal (do not special-case it open).
- `Msg::TransformDone` gets explicit arms in the **normal match** and the **`prompt` interception block**; the `minibuffer` block intercepts only key events so non-key `TransformDone` falls through to the normal arm (same as `FilterDone`/`ExportDone`).
- `transform_in_flight` is cleared on **every** `TransformDone` path (applied / discarded / error). A transform and a 4c-1 filter may both be in flight; correctness is via version-discard, no proactive cross-block.
- Status strings exactly per spec §6.2. `cargo build --workspace` zero warnings; no prior test weakened. Not-yet-wired new items carry scoped `#[allow(dead_code)]` with a "wired in Task N" note, removed when used.

---

## File Structure

- **Create:** `wordcartel/src/transform.rs` — `TransformKind`, `TransformError`, `run_transform` (typed repar wrapper), `snap_to_blocks` + `region_for_transform`, `dispatch_transform`, `apply_transform_done`-caller glue. Declared `pub mod transform;` in `wordcartel/src/lib.rs`.
- **Modify:** `wordcartel/Cargo.toml` (dep), `wordcartel/src/lib.rs` (module), `wordcartel/src/prompt.rs` (`PromptAction::Transform` + `Prompt::transform_chooser`), `wordcartel/src/registry.rs` (`transform` command id), `wordcartel/src/input.rs` (`Ctrl+T` → `transform`), `wordcartel/src/app.rs` (`Msg::TransformDone`, `apply_transform_done`, `resolve_prompt` Transform arm, the two reducer arms, `Editor.transform_in_flight`), `wordcartel/src/editor.rs` (`transform_in_flight` field).
- **Test:** `wordcartel/src/transform.rs` (wrapper, snapping, dispatch), `wordcartel/src/app.rs` (chooser, sync apply, async merge).

---

### Task 1: `repar` dependency + `run_transform` typed wrapper

**Files:**
- Modify: `wordcartel/Cargo.toml`, `wordcartel/src/lib.rs`
- Create: `wordcartel/src/transform.rs`
- Test: `wordcartel/src/transform.rs`

**Interfaces:**
- Produces:
  - `pub enum TransformKind { Reflow, Unwrap, Ventilate }` (`#[derive(Clone, Copy, Debug, PartialEq, Eq)]`), with `fn verb(self) -> &'static str` → `"--reflow"`/`"--unwrap"`/`"--ventilate"`, and `fn label(self)`/`fn past_tense(self)`/`fn gerund(self)` for status strings (`reflow`/`reflowed`/`reflowing` etc.).
  - `pub enum TransformError { Repar(String) }` (`#[derive(Debug)]`) + `impl std::fmt::Display` (`"{msg}"`) + `fn from_repar(e: repar::ParError) -> TransformError`.
  - `pub fn run_transform(kind: TransformKind, input: &str, width: u32) -> Result<String, TransformError>` — the ONLY repar-API site.
  - `pub const DEFAULT_REFLOW_WIDTH: u32 = 72;`

- [ ] **Step 1: Add the dependency.** In `wordcartel/Cargo.toml` `[dependencies]` add:
```toml
repar = { path = "../../par-command/repar" }
```
Run `cargo build -p wordcartel` to confirm it resolves. Expected: builds (repar compiles). If the path fails, run `realpath --relative-to=wordcartel ../par-command/repar` from the repo root to find the correct relative path and report it — do NOT vendor or change the version.

- [ ] **Step 2: Write the failing tests** in a new `wordcartel/src/transform.rs` (these are independent property/golden checks — they do NOT compare to a fresh in-process `repar::Options`, which would be tautological; they pin observable behavior and use `repar::display_width` for column math):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflow_wraps_long_prose_within_width() {
        let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega";
        let out = run_transform(TransformKind::Reflow, long, 72).unwrap();
        for line in out.lines() {
            assert!(repar::display_width(line) <= 72, "line over width: {line:?}");
        }
        // Round-trip back to words: unwrapping the reflow yields one line with the same words.
        let unwrapped = run_transform(TransformKind::Unwrap, &out, 72).unwrap();
        assert_eq!(unwrapped.split_whitespace().collect::<Vec<_>>(),
                   long.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn unwrap_joins_a_wrapped_paragraph_to_one_logical_line() {
        let wrapped = "one two three\nfour five six\nseven eight\n";
        let out = run_transform(TransformKind::Unwrap, wrapped, 72).unwrap();
        // One paragraph → one non-empty logical line.
        assert_eq!(out.lines().filter(|l| !l.trim().is_empty()).count(), 1);
        assert_eq!(out.split_whitespace().collect::<Vec<_>>(),
                   wrapped.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn ventilate_breaks_one_sentence_per_line() {
        let para = "First sentence here. Second sentence here. Third one here.\n";
        let out = run_transform(TransformKind::Ventilate, para, 72).unwrap();
        assert_eq!(out.lines().filter(|l| !l.trim().is_empty()).count(), 3);
    }

    #[test]
    fn markdown_mode_passes_fenced_code_through_verbatim() {
        // A long line INSIDE a fenced code block must NOT be reflowed/wrapped.
        let long_code = "let x = aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;";
        let input = format!("```\n{long_code}\n```\n");
        let out = run_transform(TransformKind::Reflow, &input, 72).unwrap();
        assert!(out.contains(long_code), "fenced code line must survive verbatim:\n{out}");
    }

    #[test]
    fn markdown_mode_leaves_heading_unwrapped() {
        let input = "# A heading that is fairly long but is a heading not prose\n\nbody text\n";
        let out = run_transform(TransformKind::Reflow, &input, 72).unwrap();
        assert!(out.contains("# A heading that is fairly long but is a heading not prose"),
                "heading must pass through:\n{out}");
    }
}
```

- [ ] **Step 3: Run to verify failure.** `cargo test -p wordcartel --lib transform::tests` → FAIL (`run_transform`/`TransformKind` not found). (After Step 4, also declare the module — until then it won't compile; that's the expected red.)

- [ ] **Step 4: Implement `transform.rs`** and declare it. In `wordcartel/src/lib.rs` add `pub mod transform;` (alongside the other `pub mod` lines). Then write:
```rust
//! In-process repar transforms (Reflow / Unwrap / Ventilate). The typed wrapper
//! `run_transform` is the ONLY place that touches repar's stringly public API.

pub const DEFAULT_REFLOW_WIDTH: u32 = 72;
/// Regions at or above this byte length run off the keystroke thread (§5.2).
pub const TRANSFORM_ASYNC_THRESHOLD: usize = 1 << 20; // 1 MiB

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformKind { Reflow, Unwrap, Ventilate }

impl TransformKind {
    fn verb(self) -> &'static str {
        match self {
            TransformKind::Reflow => "--reflow",
            TransformKind::Unwrap => "--unwrap",
            TransformKind::Ventilate => "--ventilate",
        }
    }
    /// Past-tense success word: "reflowed" / "unwrapped" / "ventilated".
    pub fn past_tense(self) -> &'static str {
        match self { Self::Reflow => "reflowed", Self::Unwrap => "unwrapped", Self::Ventilate => "ventilated" }
    }
    /// Gerund for in-progress: "reflowing" / "unwrapping" / "ventilating".
    pub fn gerund(self) -> &'static str {
        match self { Self::Reflow => "reflowing", Self::Unwrap => "unwrapping", Self::Ventilate => "ventilating" }
    }
}

#[derive(Debug)]
pub enum TransformError { Repar(String) }

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { TransformError::Repar(m) => write!(f, "{m}") }
    }
}
impl TransformError {
    fn from_repar(e: repar::ParError) -> TransformError { TransformError::Repar(e.to_string()) }
}

/// Run a repar transform over `input`, markdown-aware. Pure (no IO).
pub fn run_transform(kind: TransformKind, input: &str, width: u32) -> Result<String, TransformError> {
    let mut opts = repar::Options::new().width(width);
    // apply_par_args takes &mut self and returns PResult<()> — not chainable.
    opts.apply_par_args([kind.verb()]).map_err(TransformError::from_repar)?;
    opts.apply_fixups("markdown").map_err(TransformError::from_repar)?; // Compat::MARKDOWN
    opts.format(input).map_err(TransformError::from_repar)
}
```
(If `repar::ParError` is not the re-exported error type, check `repar::PResult`'s `Err` variant — `lib.rs` re-exports `error::{PResult, ParError}`; use `ParError`.)

- [ ] **Step 5: Run tests + full suite.** `cargo test -p wordcartel --lib transform::tests` → all pass; `cargo test --workspace` → all prior green; `cargo build --workspace` → zero warnings. (`past_tense`/`gerund`/`TRANSFORM_ASYNC_THRESHOLD`/`TransformError` are not used yet — add `#[allow(dead_code)]` with `// wired in Task 3/4` notes as needed.)

- [ ] **Step 6: Commit.**
```bash
git add wordcartel/Cargo.toml wordcartel/src/lib.rs wordcartel/src/transform.rs
git commit -m "feat(transform): repar dep + run_transform typed wrapper (Reflow/Unwrap/Ventilate)"
```

---

### Task 2: Block-tree region snapping (`snap_to_blocks` + `region_for_transform`)

The Codex-CRITICAL correctness unit: a partial selection expands to the whole top-level blocks it touches, so repar never sees a split construct.

**Files:**
- Modify: `wordcartel/src/transform.rs`
- Test: `wordcartel/src/transform.rs`

**Interfaces:**
- Consumes: `wordcartel_core::block_tree::{BlockTree, Block}` (`Block.span: std::ops::Range<usize>`, `BlockTree::top_level() -> &[Block]`), `wordcartel_core::BytePos` (= `usize`).
- Produces:
  - `pub fn snap_to_blocks(blocks: &BlockTree, from: usize, to: usize) -> std::ops::Range<usize>` — expand `[from,to)` to cover every top-level block whose span intersects it; if none intersect, return `from..to` unchanged.
  - `pub fn region_for_transform(doc: &crate::editor::Document) -> std::ops::Range<usize>` — `0..buf_len` when the primary selection is empty (whole buffer), else `snap_to_blocks(&doc.blocks, sel.from(), sel.to())`.

- [ ] **Step 1: Write the failing tests** in `transform.rs` (build real block trees via `Editor::new_from_text`, then snap):
```rust
    use crate::editor::Editor;
    use wordcartel_core::block_tree::BlockTree;

    fn blocks_of(text: &str) -> BlockTree {
        Editor::new_from_text(text, None, (80, 24)).active().document.blocks.clone()
    }

    #[test]
    fn snap_expands_mid_paragraph_selection_to_whole_paragraph() {
        let text = "alpha beta gamma\ndelta epsilon zeta\n\nsecond para\n";
        let bt = blocks_of(text);
        // Selection lands inside the first paragraph (bytes 5..9 = "beta").
        let r = snap_to_blocks(&bt, 5, 9);
        // Expanded to cover the whole first paragraph block (starts at 0).
        assert_eq!(r.start, 0);
        assert!(r.end >= "alpha beta gamma\ndelta epsilon zeta\n".len() - 1);
        // It must NOT reach into the second paragraph.
        assert!(r.end <= text.find("second").unwrap());
    }

    #[test]
    fn snap_inside_fenced_code_block_with_interior_blank_covers_whole_fence() {
        // The CRITICAL case: a blank line INSIDE a fenced code block.
        let text = "```\ncode line one\n\nstill code\n```\n\nprose after\n";
        let bt = blocks_of(text);
        let fence_start = 0;
        let fence_end = text.find("\n\nprose").unwrap() + 1; // through the closing ``` line's newline
        // A selection on "still code" (after the interior blank) must snap to the WHOLE fence,
        // not just the fragment after the blank line.
        let sel_from = text.find("still").unwrap();
        let r = snap_to_blocks(&bt, sel_from, sel_from + 5);
        assert_eq!(r.start, fence_start, "must include the opening fence");
        assert!(r.end >= fence_end - 1, "must include the closing fence");
    }

    #[test]
    fn snap_multi_block_selection_covers_all_touched_blocks() {
        let text = "para one here\n\npara two here\n\npara three\n";
        let bt = blocks_of(text);
        // Selection spans from inside para one to inside para two.
        let from = 5;
        let to = text.find("two").unwrap() + 1;
        let r = snap_to_blocks(&bt, from, to);
        assert_eq!(r.start, 0);
        assert!(r.end <= text.find("para three").unwrap());
        assert!(r.end >= text.find("para two here").unwrap() + "para two here".len());
    }

    #[test]
    fn snap_with_no_intersecting_block_returns_input_range() {
        let text = "only para\n";
        let bt = blocks_of(text);
        // A range past the end intersects nothing → unchanged.
        let r = snap_to_blocks(&bt, 100, 105);
        assert_eq!(r, 100..105);
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib transform::tests::snap` → FAIL (`snap_to_blocks` missing).

- [ ] **Step 3: Implement `snap_to_blocks` + `region_for_transform`** in `transform.rs`:
```rust
use wordcartel_core::block_tree::BlockTree;

/// Expand `[from,to)` to cover every top-level block whose span intersects it.
/// If no block intersects, the range is returned unchanged. Half-open intervals;
/// callers pass a non-empty selection (from < to). The block tree is already
/// maintained per buffer (Effort 3) — this is a bounded scan, not a parse.
pub fn snap_to_blocks(blocks: &BlockTree, from: usize, to: usize) -> std::ops::Range<usize> {
    let mut start = from;
    let mut end = to;
    let mut found = false;
    for b in blocks.top_level() {
        // intersection of [from,to) and [span.start, span.end)
        if b.span.start < to && from < b.span.end {
            if !found {
                start = b.span.start;
                end = b.span.end;
                found = true;
            } else {
                start = start.min(b.span.start);
                end = end.max(b.span.end);
            }
        }
    }
    if found { start..end } else { from..to }
}

/// The byte range a transform should reformat: whole buffer when the primary
/// selection is empty, else the selection snapped to whole blocks.
pub fn region_for_transform(doc: &crate::editor::Document) -> std::ops::Range<usize> {
    let sel = doc.selection.primary();
    let buf_len = doc.buffer.len_bytes(); // confirm: len_bytes() vs len() on the buffer type
    if sel.is_empty() {
        0..buf_len
    } else {
        snap_to_blocks(&doc.blocks, sel.from(), sel.to())
    }
}
```
(Confirm the buffer byte-length method name on `doc.buffer` — it may be `len_bytes()` or `len()`. Confirm `doc.selection.primary()` returns a `Range` with `from()/to()/is_empty()` — it does, per `wordcartel-core/src/selection.rs`. `Document` is `crate::editor::Document`; confirm `blocks`/`selection`/`buffer` field names match.)

- [ ] **Step 4: Run tests + full suite.** `cargo test -p wordcartel --lib transform::tests` → all pass; `cargo test --workspace` green; zero warnings. (`region_for_transform` unused until Task 3 → `#[allow(dead_code)]` `// wired in Task 3`.)

- [ ] **Step 5: Commit.**
```bash
git add wordcartel/src/transform.rs
git commit -m "feat(transform): markdown-structural region snapping to whole block-tree blocks"
```

---

### Task 3: Synchronous transform end-to-end (chooser + Ctrl+T + sync apply)

The first user-visible deliverable: `Ctrl+T` → `[r]eflow [u]nwrap [v]entilate` → the buffer/selection is reformatted synchronously as one undoable edit.

**Files:**
- Modify: `wordcartel/src/prompt.rs`, `wordcartel/src/registry.rs`, `wordcartel/src/input.rs`, `wordcartel/src/app.rs`, `wordcartel/src/transform.rs`
- Test: `wordcartel/src/app.rs`, `wordcartel/src/transform.rs`

**Interfaces:**
- Consumes: `run_transform`, `region_for_transform`, `TransformKind`, `DEFAULT_REFLOW_WIDTH`, `commands::build_range_replace`, `Buffer::apply`/`EditKind::Other`, the modal `Prompt`/`PromptAction`/`action_for`, `registry::{Ctx, Registry, CommandId}`, `input::key_to_command_id`.
- Produces:
  - `PromptAction::Transform(crate::transform::TransformKind)`.
  - `Prompt::transform_chooser() -> Prompt`.
  - registry command id `CommandId("transform")` → raises the chooser.
  - `Ctrl+T` → `id("transform")` in `key_to_command_id`.
  - `pub fn dispatch_transform(editor: &mut crate::editor::Editor, kind: TransformKind, msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>)` in `transform.rs` — Task 3 implements the **sync branch only** (region < `TRANSFORM_ASYNC_THRESHOLD`); the async branch is a `todo`-free fallthrough that, for now, also runs sync (Task 4 adds the threshold split). To avoid rework, implement the size check now but route both branches to the same sync helper, leaving a `// Task 4: large regions go async` marker.
  - `pub fn apply_transform_result(editor: &mut Editor, kind: TransformKind, range: Range<usize>, result: Result<String, TransformError>)` — the shared merge used by both sync and (Task 4) async, with the §6.2 status contract.

- [ ] **Step 1: Write the failing tests** in `app.rs`:
```rust
    #[test]
    fn transform_chooser_maps_keys_to_kinds() {
        use crate::prompt::{Prompt, PromptAction};
        use crate::transform::TransformKind;
        let p = Prompt::transform_chooser();
        assert_eq!(p.action_for('r'), Some(PromptAction::Transform(TransformKind::Reflow)));
        assert_eq!(p.action_for('u'), Some(PromptAction::Transform(TransformKind::Unwrap)));
        assert_eq!(p.action_for('v'), Some(PromptAction::Transform(TransformKind::Ventilate)));
        assert_eq!(p.action_for('x'), None);
    }

    #[test]
    fn ctrl_t_opens_the_transform_chooser() {
        use crate::editor::Editor;
        use crate::jobs::InlineExecutor;
        use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("hello world\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let key = Event::Key(KeyEvent { code: KeyCode::Char('t'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(key), &mut e, &reg, &ex, &clk, &tx);
        assert!(e.prompt.is_some(), "Ctrl+T must open the transform chooser");
        assert_eq!(
            e.prompt.as_ref().unwrap().action_for('r'),
            Some(crate::prompt::PromptAction::Transform(crate::transform::TransformKind::Reflow)),
        );
    }

    #[test]
    fn reflow_whole_buffer_applies_one_undoable_edit() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau\n";
        let mut e = Editor::new_from_text(long, None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        // dispatch_transform takes (editor, kind, clock, msg_tx) — see Task 3 Step 6.
        crate::transform::dispatch_transform(&mut e, TransformKind::Reflow, &TestClock(0), &tx);
        let after = e.active().document.buffer.to_string();
        assert_ne!(after, long, "reflow should rewrap the long line");
        // exactly one undo restores the original
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), long);
    }

    #[test]
    fn transform_with_identical_output_makes_no_edit() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        // Already one-sentence-per-line: ventilate is a no-op → no edit, "already" status.
        let text = "Short.\n";
        let mut e = Editor::new_from_text(text, None, (80, 24));
        let v0 = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        crate::transform::dispatch_transform(&mut e, TransformKind::Ventilate, &TestClock(0), &tx);
        assert_eq!(e.active().document.buffer.to_string(), text);
        assert_eq!(e.active().document.version, v0, "no-op transform must not bump version");
        assert!(e.status.contains("already"));
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib transform_chooser reflow_whole_buffer ctrl_t_opens transform_with_identical` → FAIL (`PromptAction::Transform`, `Prompt::transform_chooser`, `dispatch_transform`, Ctrl+T binding missing).

- [ ] **Step 3: Add `PromptAction::Transform` + `Prompt::transform_chooser`** in `prompt.rs`. Add the variant to the `PromptAction` enum:
```rust
    Transform(crate::transform::TransformKind),
```
(Ensure `PromptAction` still derives what it derived before — likely `#[derive(Clone, Copy, Debug, PartialEq, Eq)]`; `TransformKind` derives the same, so the derive still holds.) Add the constructor near `export_overwrite`:
```rust
    pub fn transform_chooser() -> Prompt {
        use crate::transform::TransformKind;
        Prompt {
            message: "transform: [r]eflow  [u]nwrap  [v]entilate".into(),
            choices: vec![
                Choice { key: 'r', label: "Reflow",    action: PromptAction::Transform(TransformKind::Reflow) },
                Choice { key: 'u', label: "Unwrap",    action: PromptAction::Transform(TransformKind::Unwrap) },
                Choice { key: 'v', label: "Ventilate", action: PromptAction::Transform(TransformKind::Ventilate) },
            ],
        }
    }
```
(`action_for(ch)` already scans `choices` for `key == ch`; `Esc`/unknown keys return `None` and are handled by the existing prompt-dismiss path. Confirm the `Prompt`/`Choice` field names match the real struct.)

- [ ] **Step 4: Register the `transform` command** in `registry.rs` `builtins()` (mirroring the existing no-arg handlers; return the same type they return — `CommandResult::Handled`):
```rust
        map.insert(CommandId("transform"), |c| {
            c.editor.prompt = Some(crate::prompt::Prompt::transform_chooser());
            CommandResult::Handled
        });
```

- [ ] **Step 5: Bind `Ctrl+T`** in `input.rs` `key_to_command_id` (the `id(...)` registry map, NOT the legacy `Command`-enum map). Add alongside the other `ctrl` chords:
```rust
        KeyCode::Char('t') if ctrl => id("transform"),
```
Add a keymap unit test mirroring the existing `keymap_*` tests asserting `Ctrl+T → Some(CommandId("transform"))`.

- [ ] **Step 6: Implement `dispatch_transform` + `apply_transform_result`** in `transform.rs`:
```rust
use std::ops::Range;

/// Run a transform over the active buffer's resolved region. Task 3: synchronous
/// (msg_tx unused until Task 4 adds the >= TRANSFORM_ASYNC_THRESHOLD off-thread branch).
/// `clock` is the same &dyn Clock that resolve_prompt receives.
pub fn dispatch_transform(
    editor: &mut crate::editor::Editor,
    kind: TransformKind,
    clock: &dyn wordcartel_core::history::Clock, // confirm the Clock trait path used by Buffer::apply
    _msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    let range = region_for_transform(&editor.active().document);
    if range.is_empty() {
        editor.status = "nothing to transform".into();
        return;
    }
    // Task 4: if range.len() >= TRANSFORM_ASYNC_THRESHOLD, snapshot + spawn + send
    //         Msg::TransformDone instead of running inline. For now, run sync.
    let input = byte_slice_to_string(editor, range.clone()); // confirm slice helper (see notes)
    let result = run_transform(kind, &input, DEFAULT_REFLOW_WIDTH);
    apply_transform_result(editor, kind, range, result, clock);
}

/// Shared merge for sync (Task 3) and async (Task 4): apply the transform output
/// as one undoable edit, with the §6.2 status contract. `range` is the byte range
/// that was transformed; `result` is the repar output for that range.
pub fn apply_transform_result(
    editor: &mut crate::editor::Editor,
    kind: TransformKind,
    range: Range<usize>,
    result: Result<String, TransformError>,
    clock: &dyn wordcartel_core::history::Clock,
) {
    match result {
        Err(e) => { editor.status = format!("transform failed: {e}"); }
        Ok(out) => {
            let current = byte_slice_to_string(editor, range.clone());
            if out == current {
                editor.status = format!("already {}", kind.past_tense());
                return;
            }
            let doc_len = editor.active().document.buffer.len_bytes(); // confirm method
            let (cs, edit) = crate::commands::build_range_replace(range.start, range.end, &out, doc_len);
            let txn = wordcartel_core::history::Transaction::new(cs);
            // end any read borrow before the &mut apply; then rebuild/ensure_visible after.
            editor.active_mut().apply(
                txn, edit, wordcartel_core::history::EditKind::Other, clock);
            crate::derive::rebuild(editor);
            crate::nav::ensure_visible(editor);
            editor.status = kind.past_tense().to_string();
        }
    }
}
```
**Implementer notes (resolve against real code):** (a) `byte_slice_to_string(editor, range)` is a stand-in for "read the active buffer's `range` to an owned `String`" — implement it by copying 4c-1's exact pattern from `dispatch_filter`/`apply_filter_done` (e.g. `editor.active().document.buffer.snapshot().byte_slice(range).to_string()`); do NOT invent a new buffer method. Likewise confirm the byte-length method (`len_bytes()` vs `len()`). (b) `clock` is threaded from `resolve_prompt` (which has `clock: &dyn Clock` in scope) — no `wall_clock()` is invented. Confirm the `Clock` trait path (`wordcartel_core::history::Clock` vs another module) against `Buffer::apply`'s signature. (c) Borrow-split discipline (4r/4c-1): compute `current`/`out`/status into locals; end the `active_mut()` borrow before `derive::rebuild(editor)`/`ensure_visible(editor)`/`editor.status`. Make it compile cleanly.

- [ ] **Step 7: Wire the `resolve_prompt` arm** in `app.rs` (`resolve_prompt` at ~line 198). Add:
```rust
        PromptAction::Transform(kind) => {
            crate::transform::dispatch_transform(editor, kind, clock, msg_tx);
        }
```
(`resolve_prompt` already has `clock` and `msg_tx` in scope; the existing tail clears `editor.prompt`.) Remove the now-used `#[allow(dead_code)]` from `dispatch_transform`/`apply_transform_result`/`region_for_transform`/`past_tense`.

- [ ] **Step 8: Run tests + full suite.** `cargo test -p wordcartel --lib` then `cargo test --workspace` → all pass (the 4 new app tests + the keymap test + prior green); `cargo build --workspace` zero warnings.

- [ ] **Step 9: Commit.**
```bash
git add wordcartel/src/prompt.rs wordcartel/src/registry.rs wordcartel/src/input.rs wordcartel/src/app.rs wordcartel/src/transform.rs
git commit -m "feat(transform): Ctrl+T modal chooser + synchronous transform applied as one undoable edit"
```

---

### Task 4: Async fallback — `Msg::TransformDone` + threshold + version-discard merge

Large whole-buffer transforms run off the keystroke thread; the result merges with version-discard, mirroring 4c-1's `FilterDone`.

**Files:**
- Modify: `wordcartel/src/editor.rs` (`transform_in_flight` field), `wordcartel/src/app.rs` (`Msg::TransformDone`, `apply_transform_done`, the two reducer arms), `wordcartel/src/transform.rs` (threshold split + thread spawn)
- Test: `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `apply_transform_result` (Task 3), `region_for_transform`, `run_transform`, `editor::by_id_mut`, `TRANSFORM_ASYNC_THRESHOLD`.
- Produces:
  - `Editor.transform_in_flight: bool` (init `false`).
  - `Msg::TransformDone { buffer_id: crate::editor::BufferId, version: u64, range: std::ops::Range<usize>, kind: crate::transform::TransformKind, result: Result<String, crate::transform::TransformError> }`.
  - `fn apply_transform_done(editor, buffer_id, version, range, kind, result, clock)` — clears `transform_in_flight`; version-discard via `by_id_mut`; on fresh applies via the Task-3 `apply_transform_result` shape (targeting `by_id_mut(buffer_id)`), on stale sets the discard status.
  - `dispatch_transform` gains the real threshold split: `range.len() >= TRANSFORM_ASYNC_THRESHOLD` → snapshot + set `transform_in_flight` + spawn thread → `Msg::TransformDone`; else sync as in Task 3.

- [ ] **Step 1: Write the failing tests** in `app.rs`:
```rust
    #[test]
    fn transformdone_applies_when_fresh() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        let mut e = Editor::new_from_text("one two three four five six seven\n", None, (80, 24));
        let id = e.active().id; let v = e.active().document.version;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let range = 0..e.active().document.buffer.to_string().len();
        let out = "one\ntwo\n".to_string(); // pretend ventilate output
        crate::app::reduce(Msg::TransformDone { buffer_id: id, version: v, range,
            kind: TransformKind::Ventilate, result: Ok(out.clone()) }, &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), out);
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "one two three four five six seven\n");
    }

    #[test]
    fn transformdone_discarded_when_version_moved() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        let mut e = Editor::new_from_text("alpha beta\n", None, (80, 24));
        let id = e.active().id; let stale = e.active().document.version;
        e.active_mut().document.version += 1; // an intervening edit
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::TransformDone { buffer_id: id, version: stale, range: 0..10,
            kind: TransformKind::Reflow, result: Ok("X".into()) }, &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "alpha beta\n", "stale result discarded");
        assert!(e.status.to_lowercase().contains("discarded"));
        assert!(!e.transform_in_flight, "in-flight cleared even on discard");
    }

    #[test]
    fn large_buffer_routes_async_and_delivers_transformdone() {
        use crate::editor::Editor;
        use crate::transform::TransformKind;
        // > 1 MiB buffer forces the async branch; we block on the channel.
        let big = "word ".repeat(300_000); // ~1.5 MB
        let mut e = Editor::new_from_text(&big, None, (80, 24));
        let (tx, rx) = std::sync::mpsc::channel::<Msg>();
        crate::transform::dispatch_transform(&mut e, TransformKind::Unwrap, &TestClock(0), &tx);
        assert!(e.transform_in_flight, "async dispatch sets the in-flight guard");
        let msg = rx.recv().expect("TransformDone must arrive");
        match msg { Msg::TransformDone { kind: TransformKind::Unwrap, result: Ok(_), .. } => {}
                    other => panic!("expected TransformDone Ok, got {other:?}") }
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib transformdone large_buffer_routes_async` → FAIL (`Msg::TransformDone`, `transform_in_flight` missing).

- [ ] **Step 3: Add `transform_in_flight`** to `Editor` (`editor.rs`): `pub transform_in_flight: bool,` init `false` in `new_from_text`.

- [ ] **Step 4: Add `Msg::TransformDone`** to the `Msg` enum (`app.rs`) with the fields in Interfaces, and a `Debug` arm (the enum has a manual `Debug` impl — add a `Msg::TransformDone { buffer_id, range, kind, .. }` arm mirroring the `FilterDone` one).

- [ ] **Step 5: Implement the threshold split + thread** in `transform.rs` `dispatch_transform`:
```rust
pub fn dispatch_transform(
    editor: &mut crate::editor::Editor,
    kind: TransformKind,
    clock: &dyn wordcartel_core::history::Clock, // confirm Clock path/trait
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    if editor.transform_in_flight {
        editor.status = "a transform is already running".into();
        return;
    }
    let range = region_for_transform(&editor.active().document);
    if range.is_empty() {
        editor.status = "nothing to transform".into();
        return;
    }
    if range.len() >= TRANSFORM_ASYNC_THRESHOLD {
        let buffer_id = editor.active().id;
        let version = editor.active().document.version;
        let snapshot = editor.active().document.buffer.snapshot(); // O(1) rope snapshot
        editor.transform_in_flight = true;
        editor.status = format!("{}…", kind.gerund());
        let range_c = range.clone();
        let msg_tx = msg_tx.clone();
        std::thread::spawn(move || {
            let input = snapshot.byte_slice(range_c.clone()).to_string(); // confirm slice API
            let result = run_transform(kind, &input, DEFAULT_REFLOW_WIDTH);
            let _ = msg_tx.send(crate::app::Msg::TransformDone {
                buffer_id, version, range: range_c, kind, result,
            });
        });
        return;
    }
    // Sync branch (Task 3).
    let input = editor.active().document.buffer.byte_slice_to_string(range.clone()); // same helper as Task 3
    let result = run_transform(kind, &input, DEFAULT_REFLOW_WIDTH);
    apply_transform_result(editor, kind, range, result, clock);
}
```
(Reuse the exact snapshot + `byte_slice(range).to_string()` pattern that 4c-1's `dispatch_filter` uses on its thread. `snapshot()` returns `ropey::Rope` (O(1)). Confirm the `Clock` trait path used by `Buffer::apply`.)

- [ ] **Step 6: Implement `apply_transform_done`** in `app.rs` and wire the two reducer arms. Add the function (mirrors `apply_filter_done`):
```rust
fn apply_transform_done(
    editor: &mut crate::editor::Editor,
    buffer_id: crate::editor::BufferId,
    version: u64,
    range: std::ops::Range<usize>,
    kind: crate::transform::TransformKind,
    result: Result<String, crate::transform::TransformError>,
    clock: &dyn wordcartel_core::history::Clock,
) {
    editor.transform_in_flight = false;
    let stale = editor.by_id(buffer_id).map(|b| b.document.version) != Some(version);
    if stale {
        editor.status = "transform discarded — buffer changed".into();
        return;
    }
    // Fresh: apply into the ORIGINATING buffer (by_id_mut(buffer_id)), not active().
    crate::transform::merge_transform_into(editor, buffer_id, kind, range, result, clock);
}
```
**Implementer note (keep DRY):** the sync path (`apply_transform_result`) targets the active buffer; the async path must target `by_id_mut(buffer_id)`. Factor the shared body into one private helper that takes the buffer id, e.g.
`fn merge_transform_into(editor, buffer_id, kind, range, result, clock)` which: on `Err` → `editor.status = format!("transform failed: {e}")`; on `Ok(out)` → read the current bytes of `range` from `by_id(buffer_id)`, if `out == current` set "already …" and return, else `build_range_replace` → `by_id_mut(buffer_id).apply(.., EditKind::Other, clock)`, then `derive::rebuild(editor)`+`ensure_visible(editor)` after the borrow ends, status = `kind.past_tense()`. Then make `apply_transform_result` (sync, Task 3) call `merge_transform_into(editor, editor.active().id, kind, range, result, clock)` so there is exactly ONE merge body. (This is a small refactor of the Task 3 function — do it here, do not duplicate the merge.)

Wire `Msg::TransformDone` in BOTH the normal match arm and the `editor.prompt.is_some()` interception block (parallel to their `FilterDone` arms), each calling `apply_transform_done(editor, buffer_id, version, range, kind, result, clock)`. (The minibuffer block intercepts only keys, so `TransformDone` falls through to the normal arm — no third arm.)

- [ ] **Step 7: Run tests + full suite (run the async test a few times).** `cargo test --workspace` then `for i in 1 2 3; do cargo test -p wordcartel --lib large_buffer_routes_async; done` → all stable; `cargo build --workspace` zero warnings. Remove any now-stale `#[allow(dead_code)]` (`transform_in_flight`, `gerund`, `TRANSFORM_ASYNC_THRESHOLD` are now used).

- [ ] **Step 8: Commit.**
```bash
git add wordcartel/src/editor.rs wordcartel/src/app.rs wordcartel/src/transform.rs
git commit -m "feat(transform): async fallback over 1 MiB + Msg::TransformDone version-discard merge"
```

---

## Self-Review (4c-2)

**Spec coverage:** §1/§3 transforms + `run_transform` wrapper (Task 1); §3.1 markdown-structural block snapping (Task 2); §4 Ctrl+T modal chooser + registry command + precedence (Task 3; precedence is automatic — chooser only opens in normal mode); §5.1 sync apply as one undoable edit (Task 3); §5.2 async threshold + version-discard merge + two reducer arms (Task 4); §6 error handling / no-op / discard (Tasks 3+4); §6.1 concurrency (Task 4: `transform_in_flight` + version-discard, no cross-block); §6.2 status strings (Tasks 3+4); §8 tests — golden/property checks + round-trip + snap + merge (Tasks 1, 2, 3, 4). ✅

**Codex spec-review fixes reflected:** region snapping is block-tree-structural, not blank-line (Task 2, with the fenced-block-with-interior-blank regression test); Ctrl+T precedence stated as automatic (Task 3); `TransformDone` wired in normal + prompt arm with minibuffer fall-through (Task 4, matching real `FilterDone`); filter/transform concurrency = independent guards + version-discard (Task 4); exact status strings (Tasks 3+4); round-trip law is a verified obligation (Task 1).

**Type consistency:** `TransformKind`/`TransformError`/`run_transform`/`DEFAULT_REFLOW_WIDTH`/`TRANSFORM_ASYNC_THRESHOLD` (Task 1) → `snap_to_blocks`/`region_for_transform` (Task 2) → `PromptAction::Transform(TransformKind)`/`Prompt::transform_chooser`/`dispatch_transform(editor,kind,clock,msg_tx)`/`apply_transform_result` (Task 3) → `Msg::TransformDone{buffer_id,version,range,kind,result}`/`transform_in_flight`/`apply_transform_done` (Task 4). `dispatch_transform`'s signature gains `clock` in Task 3 and is unchanged in Task 4. The merge factoring is called out as DRY in Task 4.

**Implementer-verify markers (not placeholders — real-code confirmations):** buffer byte-length method (`len_bytes()` vs `len()`); the `byte_slice(range).to_string()` / snapshot slice helper (copy 4c-1's `dispatch_filter`/`apply_filter_done` pattern verbatim); the `Clock` trait path used by `Buffer::apply`; `Document`/`Prompt`/`Choice` field names; the `repar` relative path and `repar::ParError` re-export. Each names exactly what to check and where the existing pattern lives.

---

## Execution Handoff

Plan complete. Recommended: **subagent-driven execution** (fresh subagent per task + per-task review), then an opus whole-branch review and a Codex pre-merge gate before merge — the same flow that shipped 4c-1.
