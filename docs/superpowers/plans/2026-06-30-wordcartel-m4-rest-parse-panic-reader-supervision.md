# M4-rest: parse-panic isolation + input-thread supervision — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Isolate an upstream markdown-parse panic so it can never crash the app or corrupt the terminal (empty-tree fallback + a panic-hook-suppressing guard), and turn silent input-reader-thread death into a clean shutdown that dumps every dirty buffer.

**Architecture:** Shell-side (`wordcartel`) except one trivial pure core constructor (`block_tree::empty_tree`). Reuses the existing `panicx::catch` boundary and `recovery::write_dump`. Parsing stays synchronous on the main thread; the parse boundary is inline in `derive::rebuild` + `Buffer::from_text`. A watchdog thread joins the input reader and surfaces `Msg::InputThreadDied`, which the run loop turns into `ExitReason::InputLost` → dump-all-dirty → non-zero exit.

**Tech Stack:** Rust, ratatui 0.30 + crossterm, `std::panic::catch_unwind`, `std::sync::mpsc`, ropey.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-06-30-wordcartel-m4-rest-parse-panic-reader-supervision-design.md` (Codex-clean).
- Gates before merge: `cargo test -p wordcartel-core -p wordcartel` green; `cargo build` and `cargo test --no-run` warning-free for the touched crate(s); NO new clippy warnings on lines you touch; NEVER run `cargo fmt`.
- House style: hand-formatted dense; em-dash `—` in prose comments, never `--`; no emoji outside the sanctioned `é`/`中`/`🙂` multibyte test palette; match surrounding style.
- Exact user-facing strings (verbatim):
  - parse-degraded status: `markdown parse failed — styling may be stale`
  - clipboard-death status: `clipboard unavailable`
  - input-loss stderr: `wcartel: input reader stopped — terminal may have closed; recovery written`
- `wordcartel-core` is `#![forbid(unsafe_code)]`. `catch_unwind` is not unsafe.
- Every commit ends with the trailers, verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01EJPWfWwutX7934kfA5kcY6
  ```

---

## File Structure

- `wordcartel-core/src/block_tree.rs` — add pure `empty_tree(len)` (Task 1).
- `wordcartel/src/panicx.rs` — add the thread-local caught-panic guard (Task 2).
- `wordcartel/src/term.rs` — hook consults the guard (Task 2).
- `wordcartel/src/derive.rs` — guarded `rebuild` + `apply_parse_result` helper (Task 3).
- `wordcartel/src/editor.rs` — `parse_degraded` field + guarded `Buffer::from_text` (Task 3).
- `wordcartel/src/recovery.rs` — `dump_all_dirty(editor, dir)` (Task 4).
- `wordcartel/src/app.rs` — `Msg::InputThreadDied`, `ExitReason`, watchdog, loop wiring (Task 5).
- `wordcartel/src/main.rs` — map `ExitReason` to the exit code (Task 5).
- `wordcartel/src/clipboard.rs` — clipboard-death status on `Set`/`Get` (Task 6).

---

### Task 1: core `block_tree::empty_tree(len)`

**Files:**
- Modify: `wordcartel-core/src/block_tree.rs` (add a pure constructor + unit test)

**Interfaces:**
- Produces: `pub fn empty_tree(len: usize) -> BlockTree` — a `Document` root spanning `0..len` with no children.

Real types (verified, all fields public): `BlockTree { pub root: Block }`, `Block { pub kind: BlockKind, pub span: Range<usize>, pub children: Vec<Block> }`, `BlockKind::Document`. `BlockTree::top_level()` returns `&self.root.children`. `role_at` defaults to `Paragraph` for any byte not inside a block.

- [ ] **Step 1: Write the failing test** (in `block_tree.rs`'s `#[cfg(test)] mod tests`)

```rust
#[test]
fn empty_tree_is_a_childless_document_root() {
    let t = empty_tree(42);
    assert_eq!(t.root.kind, BlockKind::Document);
    assert_eq!(t.root.span, 0..42);
    assert!(t.top_level().is_empty());
    // Any byte resolves to the default Paragraph role — no child span to slice.
    assert_eq!(t.role_at(0), crate::style::BlockRole::Paragraph);
    assert_eq!(t.role_at(41), crate::style::BlockRole::Paragraph);
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p wordcartel-core --lib block_tree::tests::empty_tree_is_a_childless_document_root`
Expected: FAIL — `cannot find function empty_tree`.

- [ ] **Step 3: Add the constructor** (place it near `BlockTree`'s inherent impl or the other free constructors, e.g. just after `full_parse` around block_tree.rs:321, matching surrounding style)

```rust
/// A childless `Document`-root tree spanning `0..len`. The safe fallback when a
/// parse cannot run (M4-rest): it has NO child spans, so span-slicing consumers
/// (fold / outline / nav / transform) have nothing to slice out of range, and
/// `role_at` returns the default `Paragraph` everywhere.
pub fn empty_tree(len: usize) -> BlockTree {
    BlockTree { root: Block { kind: BlockKind::Document, span: 0..len, children: Vec::new() } }
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p wordcartel-core --lib block_tree::tests::empty_tree_is_a_childless_document_root`
Expected: PASS.

- [ ] **Step 5: Gates + commit**

`cargo test -p wordcartel-core` green; `cargo build -p wordcartel-core` warning-free.
```bash
git add wordcartel-core/src/block_tree.rs
git commit -m "feat(block_tree): pure empty_tree(len) fallback constructor"   # + trailers
```

---

### Task 2: `panicx` caught-panic guard + `term.rs` hook suppression

**Files:**
- Modify: `wordcartel/src/panicx.rs` (add thread-local guard; wrap `catch`)
- Modify: `wordcartel/src/term.rs` (hook consults `caught_guard_active()`)

**Interfaces:**
- Produces: `pub(crate) fn caught_guard_active() -> bool`. `catch` gains the guard side effect (signature unchanged: `pub(crate) fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String>`).

**Why:** `catch_unwind` does NOT stop the panic hook — Rust runs the hook at the panic site before unwinding. Today every `catch` site is on a non-main thread (hook gated off), but Task 3 introduces a **main-thread** `catch` around the parse; without this guard the hook would tear down the terminal + dump on a caught parse panic.

- [ ] **Step 1: Write the failing tests** (append to `panicx.rs`'s `#[cfg(test)] mod tests`)

```rust
#[test]
fn guard_is_inactive_outside_catch_and_active_inside() {
    assert!(!caught_guard_active());
    let inside = catch(|| caught_guard_active()).unwrap();
    assert!(inside, "guard must be active inside catch");
    assert!(!caught_guard_active(), "guard restored after catch");
}

#[test]
fn guard_restores_previous_value_on_nesting() {
    let inner_seen = catch(|| {
        assert!(caught_guard_active());
        let deeper = catch(|| caught_guard_active()).unwrap();
        // after the inner catch returns, the outer guard is still active
        (deeper, caught_guard_active())
    })
    .unwrap();
    assert_eq!(inner_seen, (true, true));
    assert!(!caught_guard_active());
}

#[test]
fn guard_is_restored_even_when_the_closure_panics() {
    let _ = catch(|| panic!("boom"));
    assert!(!caught_guard_active(), "guard must reset after a caught panic");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p wordcartel --lib panicx::tests`
Expected: FAIL — `cannot find function caught_guard_active`.

- [ ] **Step 3: Implement the guard** (replace the top of `panicx.rs` — the `catch` fn — and add the guard machinery; keep `panic_message` and the existing tests)

```rust
use std::cell::Cell;

thread_local! {
    /// Set while a `catch` is active on THIS thread. The panic hook consults it
    /// (via `caught_guard_active`) to suppress its teardown for a panic that
    /// `catch` will itself handle.
    static CAUGHT_GUARD: Cell<bool> = const { Cell::new(false) };
}

/// True while a `catch` is in progress on the current thread.
pub(crate) fn caught_guard_active() -> bool {
    CAUGHT_GUARD.with(|g| g.get())
}

/// RAII: sets the guard true, restores the PREVIOUS value on drop (re-entrant safe).
struct GuardReset(bool);
impl Drop for GuardReset {
    fn drop(&mut self) {
        CAUGHT_GUARD.with(|g| g.set(self.0));
    }
}

/// Run `f`, catching a panic and returning a best-effort message instead of unwinding.
pub(crate) fn catch<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    // Establish the guard BEFORE catch_unwind so it is still live when the panic
    // hook runs at the panic site (the hook runs before unwinding reaches here).
    let _reset = CAUGHT_GUARD.with(|g| GuardReset(g.replace(true)));
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(panic_message)
}
```

- [ ] **Step 4: Suppress the hook in `term.rs`** — in `install_panic_hook`'s closure (term.rs:108), add a second early-return right after the thread gate:

```rust
            if !should_handle_panic(std::thread::current().id(), main_id) { return; }
            // A caught main-thread panic (panicx::catch) owns its own failure —
            // do NOT dump or restore the terminal for it.
            if crate::panicx::caught_guard_active() { return; }
            // Best-effort emergency dump (try_lock; never deadlock).
            crate::recovery::dump_on_panic();
```

- [ ] **Step 5: Run to verify tests pass**

Run: `cargo test -p wordcartel --lib panicx::tests`
Expected: PASS (all three new + the four existing `catch_*` tests).

- [ ] **Step 6: Gates + commit**

`cargo test -p wordcartel` green; `cargo build -p wordcartel` warning-free; no new clippy on touched lines.
```bash
git add wordcartel/src/panicx.rs wordcartel/src/term.rs
git commit -m "fix(panicx): caught-panic guard so the panic hook skips teardown for a caught main-thread panic"   # + trailers
```

---

### Task 3: parse-panic boundary — `derive::rebuild` + `Buffer::from_text` + `parse_degraded`

**Files:**
- Modify: `wordcartel/src/editor.rs` (add `Editor.parse_degraded: bool`; guard `Buffer::from_text`)
- Modify: `wordcartel/src/derive.rs` (guarded compute + `apply_parse_result` helper + test)

**Interfaces:**
- Consumes: `block_tree::empty_tree` (Task 1); `panicx::catch` (Task 2).
- Produces: `Editor.parse_degraded: bool`; `derive::apply_parse_result(editor: &mut Editor, new_len: usize, computed: Result<block_tree::BlockTree, String>) -> block_tree::BlockTree`.

Real anchors: `derive::rebuild` at derive.rs:82; the parse match at derive.rs:92-107; `Buffer::from_text` at editor.rs:125-137 (`let blocks = block_tree::full_parse_rope(&buffer.snapshot());`); `Editor` struct at editor.rs:287; `editor.status: String`. `new_rope` in rebuild is `editor.active().document.buffer.snapshot()` (a `ropey::Rope`); use `new_rope.len_bytes()` for the fallback length.

- [ ] **Step 1: Add the `parse_degraded` field** to `Editor` (editor.rs:287, after `pub status: String`):

```rust
    pub status: String,
    /// True while the last block-tree parse panicked (M4-rest). Dedupes the
    /// status notice so a persistently-panicking document does not spam it.
    pub parse_degraded: bool,
```

Initialize `parse_degraded: false` in every `Editor { … }` struct literal (the compiler will point to each construction site — there is a primary constructor plus test builders).

- [ ] **Step 2: Write the failing test** for the state-transition helper (in `derive.rs`'s `#[cfg(test)] mod tests`; use the existing test `Editor` construction pattern already used by other derive.rs tests):

```rust
#[test]
fn apply_parse_result_err_installs_empty_tree_and_sets_degraded_once() {
    let mut ed = /* existing test-editor builder */;
    ed.status.clear();
    ed.parse_degraded = false;

    // First Err: empty tree + degraded + notice.
    let t = apply_parse_result(&mut ed, 10, Err("boom".to_string()));
    assert!(ed.parse_degraded);
    assert_eq!(ed.status, "markdown parse failed — styling may be stale");
    assert_eq!(t.root.span, 0..10);
    assert!(t.top_level().is_empty());

    // Second Err while already degraded: still empty tree, notice unchanged (no spam).
    ed.status = "markdown parse failed — styling may be stale".to_string();
    let _ = apply_parse_result(&mut ed, 12, Err("again".to_string()));
    assert!(ed.parse_degraded);

    // Ok while degraded: real tree returned, degraded cleared, notice cleared.
    let real = block_tree::full_parse_rope(&ropey::Rope::from_str("# H\n"));
    let got = apply_parse_result(&mut ed, 4, Ok(real.clone()));
    assert_eq!(got, real);
    assert!(!ed.parse_degraded);
    assert_eq!(ed.status, "");
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p wordcartel --lib derive::tests::apply_parse_result_err_installs_empty_tree_and_sets_degraded_once`
Expected: FAIL — `cannot find function apply_parse_result`.

- [ ] **Step 4: Implement `apply_parse_result`** (add to `derive.rs`; `use wordcartel_core::block_tree;` is already in scope there):

```rust
/// Turn a guarded parse result into the tree to install, managing the deduped
/// parse-degraded notice. On `Err` we install the empty-tree fallback (no child
/// spans → no consumer can slice the current rope out of range) and set the
/// notice once; on `Ok` we clear the notice if it was set.
pub(crate) fn apply_parse_result(
    editor: &mut Editor,
    new_len: usize,
    computed: Result<block_tree::BlockTree, String>,
) -> block_tree::BlockTree {
    match computed {
        Ok(tree) => {
            if editor.parse_degraded {
                editor.parse_degraded = false;
                editor.status.clear();
            }
            tree
        }
        Err(_) => {
            if !editor.parse_degraded {
                editor.parse_degraded = true;
                editor.status = "markdown parse failed — styling may be stale".to_string();
            }
            block_tree::empty_tree(new_len)
        }
    }
}
```

- [ ] **Step 5: Wire the guarded compute into `rebuild`** — replace derive.rs:92-107 (the `let new_blocks = match … ; editor.active_mut().document.blocks = new_blocks;`) with:

```rust
    let new_len = new_rope.len_bytes();
    // Guard the parse: an upstream pulldown-cmark panic must not crash the app.
    // The closure borrows the editor immutably (old blocks) + the taken locals;
    // panicx::catch returns an owned Result, releasing the borrow before we
    // mutate the editor below. The main-thread caught-panic guard (panicx) keeps
    // the panic hook from tearing down the terminal.
    let computed = crate::panicx::catch(|| match (&maybe_old_rope, &maybe_edit) {
        (Some(old_rope), Some(edit)) => block_tree::incremental_update_rope(
            &editor.active().document.blocks,
            old_rope,
            edit,
            &new_rope,
        ),
        _ => block_tree::full_parse_rope(&new_rope),
    });
    let new_blocks = apply_parse_result(editor, new_len, computed);
    editor.active_mut().document.blocks = new_blocks;
```

(Note: `maybe_old_rope`/`maybe_edit` are already `.take()`-owned locals at derive.rs:89-90; the closure borrows them by reference. No hot-path clone. On a successful parse the only added cost is the `catch_unwind` frame.)

- [ ] **Step 6: Guard `Buffer::from_text`** — replace editor.rs:127 (`let blocks = block_tree::full_parse_rope(&buffer.snapshot());`) with:

```rust
        let blocks = match crate::panicx::catch(|| block_tree::full_parse_rope(&buffer.snapshot())) {
            Ok(t) => t,
            // No previous tree at construction — fall back to the empty tree so the
            // buffer still opens instead of crashing on an upstream parse panic.
            Err(_) => block_tree::empty_tree(text.len()),
        };
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p wordcartel --lib derive`
Expected: PASS (the new `apply_parse_result` test + existing derive tests). Then `cargo test -p wordcartel` green overall.

- [ ] **Step 8: Gates + commit**

Warning-free build/test-compile; no new clippy on touched lines; no `cargo fmt`.
```bash
git add wordcartel/src/derive.rs wordcartel/src/editor.rs
git commit -m "fix(derive): guard block-tree parse with empty-tree fallback + deduped degraded notice"   # + trailers
```

---

### Task 4: `recovery::dump_all_dirty(editor, dir)`

**Files:**
- Modify: `wordcartel/src/recovery.rs` (add `dump_all_dirty` + test)

**Interfaces:**
- Consumes: existing `recovery::write_dump(path: Option<&Path>, rope: &ropey::Rope, dir: &Path) -> io::Result<PathBuf>`; `crate::editor::Editor`.
- Produces: `pub fn dump_all_dirty(editor: &crate::editor::Editor, dir: &Path) -> usize` (count dumped).

Real anchors: `Editor.buffers: Vec<Buffer>` (editor.rs:288); each `Buffer` has `id` and `document`; `Document { pub buffer: TextBuffer, pub path: Option<PathBuf>, … }` with `dirty()` (`Some(version) != saved_version`, editor.rs:69); `TextBuffer::snapshot() -> ropey::Rope`. Use raw `Document::dirty()`, NOT `Editor::is_dirty` (which excludes scratch — but scratch-with-content is unsaved work we MUST preserve). `write_dump` names a path-less (scratch) dump `recovered-scratch-<pid>.md`.

- [ ] **Step 1: Write the failing test** (in `recovery.rs`'s `#[cfg(test)] mod tests`; build the editor with the existing test constructor and make buffers dirty via `Buffer::apply`, mirroring editor.rs's dirty-buffer tests; dump into a unique temp dir under `std::env::temp_dir()` and clean up):

```rust
#[test]
fn dump_all_dirty_writes_one_file_per_dirty_buffer_including_scratch() {
    // dir: a unique temp subdir (no tempfile crate in the tree).
    let dir = std::env::temp_dir().join(format!("wcartel-dumptest-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // Build an editor with: a dirty FILE buffer, a dirty SCRATCH buffer, a CLEAN buffer.
    let mut ed = /* existing test-editor builder with the three buffers described;
                    dirty ones edited via Buffer::apply so version > saved_version;
                    the clean one saved (saved_version == version) */;

    let n = dump_all_dirty(&ed, &dir);
    assert_eq!(n, 2, "two dirty buffers dumped, clean one skipped");

    let names: Vec<String> = std::fs::read_dir(&dir).unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2);
    assert!(names.iter().any(|n| n.starts_with("recovered-scratch-")),
            "the scratch buffer with content is dumped");

    std::fs::remove_dir_all(&dir).ok();
    let _ = &mut ed; // silence unused-mut if the builder doesn't need it
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel --lib recovery::tests::dump_all_dirty_writes_one_file_per_dirty_buffer_including_scratch`
Expected: FAIL — `cannot find function dump_all_dirty`.

- [ ] **Step 3: Implement `dump_all_dirty`** (add to `recovery.rs`):

```rust
/// Dump every OPEN buffer that holds unsaved work into `dir`, one 0600
/// `recovered-*.md` per buffer; returns how many were written. Used by the
/// input-loss shutdown (a controlled main-loop break, so iterating buffers is
/// safe — unlike the panic hook's conservative single try_lock `dump_on_panic`).
/// Uses raw `Document::dirty()` so a scratch buffer holding content is included
/// (its content is unsaved work); clean/empty buffers are skipped.
pub fn dump_all_dirty(editor: &crate::editor::Editor, dir: &Path) -> usize {
    let mut n = 0;
    for b in &editor.buffers {
        if b.document.dirty() {
            let rope = b.document.buffer.snapshot();
            if write_dump(b.document.path.as_deref(), &rope, dir).is_ok() {
                n += 1;
            }
        }
    }
    n
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wordcartel --lib recovery::tests::dump_all_dirty_writes_one_file_per_dirty_buffer_including_scratch`
Expected: PASS.

- [ ] **Step 5: Gates + commit**

`cargo test -p wordcartel` green; warning-free; no new clippy on touched lines.
```bash
git add wordcartel/src/recovery.rs
git commit -m "feat(recovery): dump_all_dirty — persist every unsaved buffer (incl. scratch-with-content)"   # + trailers
```

---

### Task 5: input watchdog + `Msg::InputThreadDied` + `ExitReason` + run/main wiring

**Files:**
- Modify: `wordcartel/src/app.rs` (`Msg` variant + `Debug` arm + main-match arm; `ExitReason`; input watchdog; loop interception; `run` return type + InputLost dump)
- Modify: `wordcartel/src/main.rs` (map `ExitReason` to the exit code)

**Interfaces:**
- Consumes: `recovery::dump_all_dirty` (Task 4); `swap::state_dir()`.
- Produces: `pub enum ExitReason { Normal, InputLost }` (in `app`); `Msg::InputThreadDied`; `run` returns `std::io::Result<ExitReason>`.

Real anchors: `Msg` enum app.rs:26-63; manual `Debug` impl app.rs:65-102 (exhaustive, no catch-all → needs an arm); main `match msg` app.rs:1651-1756 (exhaustive, no catch-all → needs an arm); the modal match app.rs:1403-1442 ends in `_ => {}` (harmless — the loop intercepts `InputThreadDied` before `reduce`); channels app.rs:1959; input-thread spawn app.rs:1977-1987; recv loop app.rs:2089-2093; post-loop `drop(guard); Ok(())` app.rs:2113-2123; `run` signature app.rs:1814; `main` main.rs:6-12.

- [ ] **Step 1: Write the failing test** (append to `app.rs`'s `#[cfg(test)] mod tests`) — the watchdog pattern (join the input thread, then emit `InputThreadDied`):

```rust
#[test]
fn input_watchdog_emits_input_thread_died_when_the_reader_ends() {
    let (tx, rx) = std::sync::mpsc::channel::<Msg>();
    // Stand-in for the input reader that has ended (Err from read(), or a panic).
    let reader = std::thread::spawn(|| { /* returns immediately */ });
    // The watchdog logic: join, then surface the death.
    let watch_tx = tx.clone();
    std::thread::spawn(move || {
        let _ = reader.join();
        let _ = watch_tx.send(Msg::InputThreadDied);
    })
    .join()
    .unwrap();
    assert!(matches!(rx.recv().unwrap(), Msg::InputThreadDied));
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel --lib tests::input_watchdog_emits_input_thread_died_when_the_reader_ends`
Expected: FAIL — no variant `Msg::InputThreadDied`.

- [ ] **Step 3: Add the `Msg` variant + `ExitReason`** — in the `Msg` enum (after `Tick,` at app.rs:62):

```rust
    Tick,
    /// The input reader thread ended (Err from read(), or a panic). Surfaced by
    /// the input watchdog; the run loop turns it into a clean InputLost shutdown.
    InputThreadDied,
```

Add the `Debug` arm (after the `Msg::Tick => …` arm at app.rs:101):

```rust
            Msg::Tick => f.write_str("Tick"),
            Msg::InputThreadDied => f.write_str("InputThreadDied"),
```

Add `ExitReason` near the top of `app.rs` (by the `Msg` enum):

```rust
/// Why the run loop exited. Drives the process exit code in `main`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Normal,
    InputLost,
}
```

- [ ] **Step 4: Add the main-match arm** — the exhaustive `match msg` at app.rs:1651-1756 needs an arm (the loop intercepts `InputThreadDied` before `reduce`, so this is defensive/unreachable). After `Msg::ClipboardAvailability(ok) => …` (app.rs:1755):

```rust
        Msg::ClipboardAvailability(ok) => apply_clipboard_availability(editor, ok),
        // Intercepted in the run loop before `reduce` (see run()); unreachable here.
        // Arm required only for exhaustiveness. Do not process the shutdown here.
        Msg::InputThreadDied => {}
```

- [ ] **Step 5: Capture the input handle + spawn the watchdog** — replace the input-thread block at app.rs:1977-1987 with:

```rust
    // Input thread + watchdog. The reader blocks on read() and forwards events;
    // if it ever ends (Err from read(), or a panic), the watchdog surfaces
    // Msg::InputThreadDied so the loop shuts down cleanly instead of hanging
    // (other Sender<Msg> clones keep msg_rx alive, so its disconnect never fires).
    {
        let input_tx = msg_tx.clone();
        let input_handle = std::thread::Builder::new()
            .name("wcartel-input".into())
            .spawn(move || {
                while let Ok(ev) = crossterm::event::read() {
                    if input_tx.send(Msg::Input(ev)).is_err() { break; }
                }
            })
            .expect("spawn input thread");
        let watch_tx = msg_tx.clone();
        std::thread::Builder::new()
            .name("wcartel-input-watchdog".into())
            .spawn(move || {
                let _ = input_handle.join(); // unblocks on ANY reader end (Ok or panic)
                let _ = watch_tx.send(Msg::InputThreadDied);
            })
            .expect("spawn input watchdog");
    }
```

- [ ] **Step 6: Intercept in the loop + thread `ExitReason`** — declare a reason before the loop (near the other loop-locals just before `loop {`):

```rust
    let mut exit_reason = ExitReason::Normal;
```

Immediately after the `let msg = match msg_rx.recv_timeout(timeout) { … };` block (app.rs:2089-2093), before `reduce` is called:

```rust
        // Input-reader death: shut down cleanly BEFORE any modal/reduce handling
        // (the modal match would otherwise swallow it via its `_ => {}`).
        if let Msg::InputThreadDied = msg {
            exit_reason = ExitReason::InputLost;
            break;
        }
```

- [ ] **Step 7: Dump-all-dirty + widen `run`** — after the loop, before the existing post-loop persist/`drop(guard)` (app.rs:2113), add the input-loss dump; then change the final return:

```rust
    // Input-loss shutdown: persist every dirty buffer non-interactively (the
    // interactive quit-drain can't run — input is gone). Controlled break, so
    // iterating buffers is safe.
    if exit_reason == ExitReason::InputLost {
        if let Ok(dir) = crate::swap::state_dir() {
            crate::recovery::dump_all_dirty(&editor, &dir);
        }
    }
```

Change `run`'s signature (app.rs:1814) to `pub fn run(cli: config::Cli) -> std::io::Result<ExitReason>`, and change the final `Ok(())` (app.rs:2123) to `Ok(exit_reason)`. Update any other `Ok(())` early-returns in `run` to `Ok(ExitReason::Normal)` (the compiler lists them). `?`-operator error returns are unchanged (still `Err`).

- [ ] **Step 8: Map the exit code in `main.rs`** — replace `main.rs:6-12`:

```rust
fn main() {
    let cli = wordcartel::config::parse_cli(std::env::args());
    match wordcartel::app::run(cli) {
        Ok(wordcartel::app::ExitReason::Normal) => {}
        Ok(wordcartel::app::ExitReason::InputLost) => {
            eprintln!("wcartel: input reader stopped — terminal may have closed; recovery written");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("wcartel: {e}");
            std::process::exit(1);
        }
    }
}
```

- [ ] **Step 9: Run tests**

Run: `cargo test -p wordcartel` — the new watchdog test passes; the whole shell suite stays green; `cargo build -p wordcartel` + `cargo test --no-run -p wordcartel` warning-free (watch for other `run()` call sites — e.g. tests — that now see `ExitReason`; update them to ignore/match the returned reason).

- [ ] **Step 10: Gates + commit**

No new clippy on touched lines; no `cargo fmt`.
```bash
git add wordcartel/src/app.rs wordcartel/src/main.rs
git commit -m "feat(app): input watchdog -> InputThreadDied -> clean InputLost shutdown (dump all dirty)"   # + trailers
```

---

### Task 6: clipboard-death status notice

**Files:**
- Modify: `wordcartel/src/clipboard.rs` (`drain_clipboard_intents`: set status on a dead worker)

**Interfaces:**
- Consumes: `editor.status` (public); the existing `drain_clipboard_intents(editor, out, clip_tx, msg_tx)` at clipboard.rs:34.

Real anchors: `Set` send is silently ignored at clipboard.rs:45 (`let _ = clip_tx.send(ClipReq::Set(text));`); `Get` send-Err is detected at clipboard.rs:48 and falls back to `Msg::ClipboardPaste { text: None }`.

- [ ] **Step 1: Write the failing test** (in `clipboard.rs`'s `#[cfg(test)] mod tests`; drop the receiver so sends fail; use the existing test `Editor` construction):

```rust
#[test]
fn a_dead_clipboard_worker_sets_the_status_notice() {
    let (clip_tx, clip_rx) = std::sync::mpsc::channel::<ClipReq>();
    drop(clip_rx); // worker is gone: every send now errors
    let (msg_tx, _msg_rx) = std::sync::mpsc::channel::<crate::app::Msg>();
    let mut out: Vec<u8> = Vec::new();
    let mut ed = /* existing test-editor builder */;

    // A pending Get with no worker -> notice + None fallback.
    ed.clipboard_get_pending = Some(/* a PasteIntent{ id, buffer_id } via the existing helper */);
    drain_clipboard_intents(&mut ed, &mut out, &clip_tx, &msg_tx);
    assert_eq!(ed.status, "clipboard unavailable");

    // A pending Set with no worker -> notice.
    ed.status.clear();
    ed.clipboard_sync_request = Some("hello".to_string());
    drain_clipboard_intents(&mut ed, &mut out, &clip_tx, &msg_tx);
    assert_eq!(ed.status, "clipboard unavailable");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p wordcartel --lib clipboard::tests::a_dead_clipboard_worker_sets_the_status_notice`
Expected: FAIL — status is empty (no notice today).

- [ ] **Step 3: Implement** — in `drain_clipboard_intents` (clipboard.rs:40-56), set the notice on both paths:

```rust
    if let Some(text) = editor.clipboard_sync_request.take() {
        if let Some(bytes) = osc52_set(&text) {
            let _ = out.write_all(&bytes);
            let _ = out.flush();
        }
        if clip_tx.send(ClipReq::Set(text)).is_err() {
            editor.status = "clipboard unavailable".to_string();
        }
    }
    if let Some(pi) = editor.clipboard_get_pending.take() {
        if clip_tx.send(ClipReq::Get { id: pi.id, buffer_id: pi.buffer_id }).is_err() {
            // No worker (tests / shutdown): notify, then fall back to the register paste path.
            editor.status = "clipboard unavailable".to_string();
            let _ = msg_tx.send(crate::app::Msg::ClipboardPaste {
                id: pi.id,
                buffer_id: pi.buffer_id,
                text: None,
            });
        }
    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p wordcartel --lib clipboard::tests::a_dead_clipboard_worker_sets_the_status_notice`
Expected: PASS.

- [ ] **Step 5: Gates + commit**

`cargo test -p wordcartel` green; warning-free; no new clippy on touched lines.
```bash
git add wordcartel/src/clipboard.rs
git commit -m "feat(clipboard): surface 'clipboard unavailable' when the worker is gone (Set + Get)"   # + trailers
```

---

## Self-Review

**Spec coverage:**
- Goal 1 parse boundary → Tasks 1 (empty_tree), 2 (hook suppression — the load-bearing Critical), 3 (guarded rebuild + from_text + deduped notice). ✓
- Goal 2a input supervision → Task 5 (watchdog, InputThreadDied, ExitReason, run/main). ✓
- Goal 2a all-dirty dump → Task 4 (dump_all_dirty) consumed by Task 5. ✓
- Goal 2b clipboard notice → Task 6. ✓
- empty-tree fallback uniform (both rebuild + from_text) → Task 3 Steps 5 & 6. ✓
- `Msg` exhaustive obligations (Debug + main match) + modal bypass (loop interception) → Task 5 Steps 3, 4, 6. ✓
- exit-code ordering (dump → guard drop → process::exit in main) → Task 5 Steps 7, 8. ✓
- scratch predicate = raw `Document::dirty()` → Task 4 Step 3 + test. ✓

**Type consistency:** `empty_tree(usize) -> BlockTree`, `apply_parse_result(&mut Editor, usize, Result<BlockTree,String>) -> BlockTree`, `dump_all_dirty(&Editor, &Path) -> usize`, `ExitReason`, `Msg::InputThreadDied`, `caught_guard_active() -> bool` — used consistently across tasks and match the real signatures read from source.

**Placeholder scan:** the two `/* existing test-editor builder */` markers are deliberate — the implementer must use the crate's real test constructor (each modifying task already has sibling tests using it); the assertion logic and exact values are fully specified. No logic is left as a placeholder.

**Ordering:** 1 → 2 → 3 (3 consumes 1+2); 4 → 5 (5 consumes 4); 6 independent. Each task compiles + tests green on its own.
