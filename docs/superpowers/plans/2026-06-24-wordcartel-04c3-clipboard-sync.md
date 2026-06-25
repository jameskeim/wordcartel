# Wordcartel 4c-3 — System Clipboard Sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sync the existing internal copy/cut/paste register to/from the OS clipboard — `arboard` (off-thread worker) + OSC 52 (inline, SSH) for writes; async OS-read + register fallback + bracketed paste for reads — without ever blocking the main loop or breaking editing.

**Architecture:** The in-process `Register` stays source of truth. A `ClipboardBackend` trait (Arboard/Null/Fake) lives behind a long-lived worker thread that inits arboard *off the startup path* and serves Set/Get over an unbounded channel, replying via `Msg::ClipboardPaste`. Commands set intent fields on `Editor`; a pure `drain_clipboard_intents` seam (called by `run()` after `reduce`) emits OSC 52 to the terminal backend writer and sends worker requests. Paste is async and buffer-targeted; bracketed paste inserts per UI mode.

**Tech Stack:** Rust; `arboard` (text-only, `wayland-data-control`); crossterm 0.28 bracketed paste; a self-contained base64 + OSC 52 encoder; the existing `Msg`/`reduce` loop, `Register`, `build_range_replace`, `TerminalGuard`.

**Spec:** `docs/superpowers/specs/2026-06-24-wordcartel-04c3-clipboard-sync-design.md` (Codex-reviewed: 1 crit + 6 imp + 3 min applied).

## Global Constraints

- `#![forbid(unsafe_code)]` in the shell crate; `wordcartel-core` stays IO/thread-free (all clipboard IO/threading is in `wordcartel/src/clipboard.rs`). `arboard` added to `wordcartel/Cargo.toml` ONLY.
- Exact dep: `arboard = { version = "3", default-features = false, features = ["wayland-data-control"] }` (confirm version/feature at impl time; `realpath` not needed — it's a registry crate).
- **The internal `Register` is always source of truth; OS sync is degradable and must NEVER block the main loop or break editing. Never `unwrap` a clipboard op.**
- **Responsiveness invariants:** arboard init happens INSIDE the worker (never on the startup/main path); the `ClipReq` channel is **unbounded** (main-loop send never blocks); OSC 52 writes go through the terminal backend writer (not a stray `io::stdout()`), emitted after `reduce`, before the frame draw.
- **OS writes happen only on explicit Copy/Cut.** Paste paths may update the in-process register but never re-emit to the OS (no feedback loop).
- **Shutdown ordering:** the terminal is restored FIRST (existing teardown/guard order), then the clipboard worker is signalled/dropped; the worker is **detached** (never joined on the exit path) so a blocked arboard `get`/`Drop` can't hold exit hostage (the 4b-1 lesson).
- Constants: `OSC52_MAX_ENCODED = 100_000` (cap on the base64 payload), `PASTE_MAX_BYTES = 8 * 1024 * 1024`. OSC 52 terminator = ST (`\x1b\\`).
- `cargo build --workspace` zero warnings; not-yet-wired items carry scoped `#[allow(dead_code)]` with a `// wired in Task N` note (removed when used). No prior test weakened.

---

## File Structure

- **Create:** `wordcartel/src/clipboard.rs` — `ClipboardBackend` (+ Arboard/Null/Fake), `base64_encode`, `osc52_set`, `ClipReq`, `PasteIntent`, `next_paste_id`, `spawn_worker`, `drain_clipboard_intents`. Declared `pub mod clipboard;` in `lib.rs`.
- **Modify:** `wordcartel/Cargo.toml` (dep), `wordcartel/src/lib.rs` (module), `wordcartel/src/editor.rs` (3 intent fields), `wordcartel/src/commands.rs` (Copy/Cut/Paste set intent), `wordcartel/src/app.rs` (`Msg::ClipboardPaste`/`ClipboardAvailability` + Debug arms + reduce arms + `Event::Paste` arm + `run()` worker/drain wiring), `wordcartel/src/term.rs` (bracketed paste enable/disable).
- **Test:** `wordcartel/src/clipboard.rs` (encoder, fake backend, drain seam), `wordcartel/src/app.rs` (async paste, availability, bracketed paste per mode, size cap).

---

### Task 1: `arboard` dep + `ClipboardBackend` + OSC 52 / base64 encoder

**Files:**
- Modify: `wordcartel/Cargo.toml`, `wordcartel/src/lib.rs`
- Create: `wordcartel/src/clipboard.rs`
- Test: `wordcartel/src/clipboard.rs`

**Interfaces:**
- Produces:
  - `pub trait ClipboardBackend: Send { fn set(&mut self, text: &str); fn get(&mut self) -> Option<String>; }`
  - `pub struct ArboardBackend { cb: arboard::Clipboard }` (constructed via `ArboardBackend::try_new() -> Option<ArboardBackend>`), `pub struct NullBackend;`, `pub struct FakeBackend { pub slot: Option<String> }`.
  - `pub const OSC52_MAX_ENCODED: usize = 100_000;`
  - `pub fn osc52_set(text: &str) -> Option<Vec<u8>>` (None when the base64 exceeds the cap; else the full `ESC ] 52 ; c ; <b64> ESC \` byte sequence).

- [ ] **Step 1: Add the dependency + module.** In `wordcartel/Cargo.toml` `[dependencies]`:
```toml
arboard = { version = "3", default-features = false, features = ["wayland-data-control"] }
```
In `wordcartel/src/lib.rs` add `pub mod clipboard;`. Run `cargo build -p wordcartel` to confirm arboard resolves. (If the feature name differs in the resolved 3.x, check `cargo doc -p arboard` / its Cargo.toml and use the current name — do NOT drop Wayland support.)

- [ ] **Step 2: Write the failing tests** in `clipboard.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"hi"), "aGk=");
    }

    #[test]
    fn osc52_frames_with_st_terminator() {
        assert_eq!(osc52_set("hi").unwrap(), b"\x1b]52;c;aGk=\x1b\\".to_vec());
    }

    #[test]
    fn osc52_skips_oversize_payload() {
        // A raw string whose base64 exceeds the cap → None (skip OSC 52).
        let big = "a".repeat(OSC52_MAX_ENCODED); // base64 ~4/3 larger → over cap
        assert!(osc52_set(&big).is_none());
    }

    #[test]
    fn fake_backend_round_trips() {
        let mut b = FakeBackend { slot: None };
        assert_eq!(b.get(), None);
        b.set("x");
        assert_eq!(b.get(), Some("x".to_string()));
    }

    #[test]
    fn null_backend_is_inert() {
        let mut b = NullBackend;
        b.set("x");
        assert_eq!(b.get(), None);
    }
}
```

- [ ] **Step 3: Run to verify failure.** `cargo test -p wordcartel --lib clipboard::tests` → FAIL (items missing).

- [ ] **Step 4: Implement** in `clipboard.rs`:
```rust
//! System-clipboard sync around the in-process Register. The Register is always
//! source of truth; everything here is best-effort and must never block the loop.

pub const OSC52_MAX_ENCODED: usize = 100_000;

pub trait ClipboardBackend: Send {
    fn set(&mut self, text: &str);
    fn get(&mut self) -> Option<String>;
}

pub struct ArboardBackend { cb: arboard::Clipboard }
impl ArboardBackend {
    /// Init arboard; None if no display / unsupported (caller falls back to Null).
    pub fn try_new() -> Option<ArboardBackend> {
        arboard::Clipboard::new().ok().map(|cb| ArboardBackend { cb })
    }
}
impl ClipboardBackend for ArboardBackend {
    fn set(&mut self, text: &str) { let _ = self.cb.set_text(text.to_owned()); } // swallow errors
    fn get(&mut self) -> Option<String> { self.cb.get_text().ok() }
}

pub struct NullBackend;
impl ClipboardBackend for NullBackend {
    fn set(&mut self, _text: &str) {}
    fn get(&mut self) -> Option<String> { None }
}

pub struct FakeBackend { pub slot: Option<String> }
impl ClipboardBackend for FakeBackend {
    fn set(&mut self, text: &str) { self.slot = Some(text.to_owned()); }
    fn get(&mut self) -> Option<String> { self.slot.clone() }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
pub(crate) fn base64_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | (b2 as u32);
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { B64[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[(n & 63) as usize] as char } else { '=' });
    }
    out
}

/// OSC 52 "set clipboard" sequence (ST-terminated). None when over the encoded cap.
pub fn osc52_set(text: &str) -> Option<Vec<u8>> {
    let b64 = base64_encode(text.as_bytes());
    if b64.len() > OSC52_MAX_ENCODED { return None; }
    let mut v = Vec::with_capacity(b64.len() + 9);
    v.extend_from_slice(b"\x1b]52;c;");
    v.extend_from_slice(b64.as_bytes());
    v.extend_from_slice(b"\x1b\\");
    Some(v)
}
```
(Confirm `arboard::Clipboard::{new, set_text, get_text}` signatures — `set_text(impl Into<String>)`, `get_text() -> Result<String>`. Errors are swallowed per the never-block/never-unwrap rule.)

- [ ] **Step 5: Run tests + suite.** `cargo test -p wordcartel --lib clipboard::tests` → pass; `cargo test --workspace` green; `cargo build --workspace` zero warnings. (`ArboardBackend`/`NullBackend`/`osc52_set` are unused until Task 2/4 → scoped `#[allow(dead_code)] // wired in Task 2/4`.)

- [ ] **Step 6: Commit.**
```bash
git add wordcartel/Cargo.toml wordcartel/src/lib.rs wordcartel/src/clipboard.rs
git commit -m "feat(clipboard): arboard dep + ClipboardBackend (Arboard/Null/Fake) + OSC 52/base64 encoder"
```

---

### Task 2: Worker + `ClipReq`/`Msg` + the `drain_clipboard_intents` seam

**Files:**
- Modify: `wordcartel/src/clipboard.rs`, `wordcartel/src/app.rs` (Msg variants + Debug arms), `wordcartel/src/editor.rs` (intent fields)
- Test: `wordcartel/src/clipboard.rs`

**Interfaces:**
- Consumes: `ClipboardBackend`, `ArboardBackend`, `NullBackend`, `osc52_set` (Task 1).
- Produces:
  - `pub enum ClipReq { Set(String), Get { id: u64, buffer_id: crate::editor::BufferId }, Shutdown }`
  - `pub struct PasteIntent { pub id: u64, pub buffer_id: crate::editor::BufferId }`
  - `pub fn next_paste_id() -> u64` (monotonic via a `static AtomicU64`).
  - `Msg::ClipboardPaste { id: u64, buffer_id: crate::editor::BufferId, text: Option<String> }` and `Msg::ClipboardAvailability(bool)` (app.rs).
  - `Editor.clipboard_sync_request: Option<String>`, `Editor.clipboard_get_pending: Option<crate::clipboard::PasteIntent>`, `Editor.clipboard_notice_shown: bool` (init `None`/`None`/`false`).
  - `pub fn spawn_worker(msg_tx: std::sync::mpsc::Sender<crate::app::Msg>) -> std::sync::mpsc::Sender<ClipReq>`
  - `pub fn drain_clipboard_intents(editor: &mut crate::editor::Editor, out: &mut impl std::io::Write, clip_tx: &std::sync::mpsc::Sender<ClipReq>, msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>)`

- [ ] **Step 1: Add the `Editor` fields** (`editor.rs`): in the struct, `pub clipboard_sync_request: Option<String>,`, `pub clipboard_get_pending: Option<crate::clipboard::PasteIntent>,`, `pub clipboard_notice_shown: bool,`; in `new_from_text`'s initializer list add `clipboard_sync_request: None, clipboard_get_pending: None, clipboard_notice_shown: false,`.

- [ ] **Step 2: Add the `Msg` variants + Debug arms** (`app.rs`): in `pub enum Msg` add
```rust
    ClipboardPaste { id: u64, buffer_id: crate::editor::BufferId, text: Option<String> },
    ClipboardAvailability(bool),
```
In the manual `Debug` `match self`, add (mirroring the other arms — do NOT print the full `text`):
```rust
            Msg::ClipboardPaste { id, buffer_id, text } => f.debug_struct("ClipboardPaste")
                .field("id", id).field("buffer_id", buffer_id)
                .field("has_text", &text.is_some()).finish(),
            Msg::ClipboardAvailability(ok) => f.debug_tuple("ClipboardAvailability").field(ok).finish(),
```

- [ ] **Step 3: Write the failing tests** in `clipboard.rs` (the drain seam — fully testable with a `Vec<u8>` + fake channels, no worker/terminal):
```rust
    #[test]
    fn drain_copy_emits_osc52_and_set_request() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        e.clipboard_sync_request = Some("hi".into());
        let (clip_tx, clip_rx) = std::sync::mpsc::channel();
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel();
        let mut out: Vec<u8> = Vec::new();
        drain_clipboard_intents(&mut e, &mut out, &clip_tx, &msg_tx);
        assert_eq!(out, b"\x1b]52;c;aGk=\x1b\\".to_vec(), "OSC 52 written to the terminal writer");
        match clip_rx.try_recv() { Ok(ClipReq::Set(s)) => assert_eq!(s, "hi"), o => panic!("{o:?}") }
        assert!(e.clipboard_sync_request.is_none(), "intent cleared");
    }

    #[test]
    fn drain_paste_sends_get_request() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let bid = e.active().id;
        e.clipboard_get_pending = Some(PasteIntent { id: 7, buffer_id: bid });
        let (clip_tx, clip_rx) = std::sync::mpsc::channel();
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel();
        let mut out: Vec<u8> = Vec::new();
        drain_clipboard_intents(&mut e, &mut out, &clip_tx, &msg_tx);
        match clip_rx.try_recv() {
            Ok(ClipReq::Get { id, buffer_id }) => { assert_eq!(id, 7); assert_eq!(buffer_id, bid); }
            o => panic!("{o:?}"),
        }
        assert!(e.clipboard_get_pending.is_none());
    }

    #[test]
    fn drain_paste_with_dead_worker_falls_back_to_none_msg() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let bid = e.active().id;
        e.clipboard_get_pending = Some(PasteIntent { id: 1, buffer_id: bid });
        let (clip_tx, clip_rx) = std::sync::mpsc::channel::<ClipReq>();
        drop(clip_rx); // worker gone → send fails
        let (msg_tx, msg_rx) = std::sync::mpsc::channel();
        let mut out: Vec<u8> = Vec::new();
        drain_clipboard_intents(&mut e, &mut out, &clip_tx, &msg_tx);
        match msg_rx.try_recv() {
            Ok(crate::app::Msg::ClipboardPaste { text: None, buffer_id, .. }) => assert_eq!(buffer_id, bid),
            o => panic!("{o:?}"),
        }
    }
```

- [ ] **Step 4: Run to verify failure.** `cargo test -p wordcartel --lib clipboard::tests::drain` → FAIL.

- [ ] **Step 5: Implement** `PasteIntent`, `next_paste_id`, `ClipReq`, `drain_clipboard_intents`, `spawn_worker` in `clipboard.rs`:
```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Debug)]
pub struct PasteIntent { pub id: u64, pub buffer_id: crate::editor::BufferId }

pub enum ClipReq { Set(String), Get { id: u64, buffer_id: crate::editor::BufferId }, Shutdown }

static PASTE_SEQ: AtomicU64 = AtomicU64::new(1);
pub fn next_paste_id() -> u64 { PASTE_SEQ.fetch_add(1, Ordering::Relaxed) }

/// Called by run() after reduce, before the frame draw. `out` is the terminal
/// backend writer (a Vec<u8> in tests). Never blocks: the channel is unbounded.
pub fn drain_clipboard_intents(
    editor: &mut crate::editor::Editor,
    out: &mut impl std::io::Write,
    clip_tx: &Sender<ClipReq>,
    msg_tx: &Sender<crate::app::Msg>,
) {
    if let Some(text) = editor.clipboard_sync_request.take() {
        if let Some(bytes) = osc52_set(&text) {
            let _ = out.write_all(&bytes);
            let _ = out.flush();
        }
        let _ = clip_tx.send(ClipReq::Set(text));
    }
    if let Some(pi) = editor.clipboard_get_pending.take() {
        if clip_tx.send(ClipReq::Get { id: pi.id, buffer_id: pi.buffer_id }).is_err() {
            // No worker (tests / shutdown): fall back to the register paste path.
            let _ = msg_tx.send(crate::app::Msg::ClipboardPaste { id: pi.id, buffer_id: pi.buffer_id, text: None });
        }
    }
}

/// Spawn the long-lived clipboard worker. arboard is initialized INSIDE the worker
/// (off the startup path); availability is reported once via Msg::ClipboardAvailability.
pub fn spawn_worker(msg_tx: Sender<crate::app::Msg>) -> Sender<ClipReq> {
    let (tx, rx) = std::sync::mpsc::channel::<ClipReq>();
    std::thread::Builder::new().name("wcartel-clipboard".into()).spawn(move || {
        let mut backend: Box<dyn ClipboardBackend> = match ArboardBackend::try_new() {
            Some(b) => { let _ = msg_tx.send(crate::app::Msg::ClipboardAvailability(true)); Box::new(b) }
            None => { let _ = msg_tx.send(crate::app::Msg::ClipboardAvailability(false)); Box::new(NullBackend) }
        };
        while let Ok(req) = rx.recv() {
            match req {
                ClipReq::Set(s) => backend.set(&s),
                ClipReq::Get { id, buffer_id } => {
                    let text = backend.get().filter(|s| !s.is_empty());
                    let _ = msg_tx.send(crate::app::Msg::ClipboardPaste { id, buffer_id, text });
                }
                ClipReq::Shutdown => break,
            }
        }
    }).expect("spawn clipboard worker");
    tx
}
```
(`spawn_worker`/`next_paste_id` are unused until Task 3/4 → scoped `#[allow(dead_code)]`. The new `Msg` variants are constructed in tests now, so no dead-code there. Note: the worker is detached — `spawn(...)` handle dropped.)

- [ ] **Step 6: Update non-exhaustive matches.** The new `Msg` variants break any `match msg` without a wildcard. For THIS task add a temporary no-op arm `Msg::ClipboardPaste { .. } | Msg::ClipboardAvailability(_) => {}` in the **normal match** (Task 3 fills it in). The `editor.prompt.is_some()` block and the minibuffer block already end with `_ => {}`, so they compile as-is now; Task 3 adds the real arms to the prompt block. (Confirm by building.)

- [ ] **Step 7: Run tests + suite.** `cargo test -p wordcartel --lib clipboard::tests` → pass; `cargo test --workspace` green; zero warnings.

- [ ] **Step 8: Commit.**
```bash
git add wordcartel/src/clipboard.rs wordcartel/src/app.rs wordcartel/src/editor.rs
git commit -m "feat(clipboard): worker (off-startup arboard init) + ClipReq/Msg + drain_clipboard_intents seam"
```

---

### Task 3: Copy/Cut/Paste wiring + async paste merge + availability notice

**Files:**
- Modify: `wordcartel/src/commands.rs` (Copy/Cut/Paste), `wordcartel/src/app.rs` (reduce arms + shared paste helper)
- Test: `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `clipboard::{PasteIntent, next_paste_id}`, `commands::build_range_replace`, `register::Register`, `editor::by_id_mut`, `Buffer::apply`/`EditKind::Other`, `PASTE_MAX_BYTES`.
- Produces:
  - Copy/Cut set `editor.clipboard_sync_request = Some(text)`.
  - Paste sets `editor.clipboard_get_pending = Some(PasteIntent { id: next_paste_id(), buffer_id: editor.active().id })` and does NOT paste inline.
  - `fn insert_paste_text(editor: &mut Editor, buffer_id: BufferId, text: &str, clock: &dyn Clock) -> bool` (the single paste-insert path: size-cap → status+false; else CUA replace-selection insert as one `EditKind::Other` edit into `by_id_mut(buffer_id)`; rebuild/ensure_visible only when that buffer is active).
  - reduce arms for `Msg::ClipboardPaste` (OS text or register fallback via `insert_paste_text`) and `Msg::ClipboardAvailability` (one-time notice).

- [ ] **Step 1: Write the failing tests** in `app.rs`:
```rust
    #[test]
    fn copy_sets_register_and_sync_request() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("hello\n", None, (80, 24));
        // select "hello" (0..5)
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(0, 5);
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let ctrl_c = Event::Key(KeyEvent { code: KeyCode::Char('c'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(ctrl_c), &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.register.get(), Some("hello"));
        assert_eq!(e.clipboard_sync_request.as_deref(), Some("hello"));
    }

    #[test]
    fn paste_keypress_sets_intent_not_inline_paste() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.register.set("Z".into());
        let before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        let ctrl_v = Event::Key(KeyEvent { code: KeyCode::Char('v'), modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press, state: KeyEventState::NONE });
        crate::app::reduce(Msg::Input(ctrl_v), &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), before, "Ctrl+V no longer pastes inline");
        assert!(e.clipboard_get_pending.is_some(), "Ctrl+V sets a paste intent");
    }

    #[test]
    fn clipboardpaste_some_inserts_os_text_one_undo() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1); // caret after 'a'
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXYb\n");
        assert_eq!(e.register.get(), Some("XY"), "OS text updates the register");
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");
    }

    #[test]
    fn clipboardpaste_none_falls_back_to_register() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        e.register.set("R".into());
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: None }, &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aRb\n", "None → register fallback");
    }

    #[test]
    fn clipboardpaste_none_empty_register_is_noop() {
        // Preserves the old paste_on_empty_register_is_noop coverage at the reduce layer.
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: None }, &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "ab\n", "empty register → no change");
    }

    #[test]
    fn clipboardpaste_replaces_active_selection() {
        // Preserves the old paste_over_selection_replaces coverage (CUA replace) at the reduce layer.
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("abcd\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::range(1, 3); // select "bc"
        let bid = e.active().id;
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: bid, text: Some("XY".into()) }, &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXYd\n", "selection replaced by pasted text");
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "abcd\n");
    }

    #[test]
    fn clipboardpaste_for_missing_buffer_is_noop() {
        use crate::editor::{Editor, BufferId}; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardPaste { id: 1, buffer_id: BufferId(99999), text: Some("X".into()) }, &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "ab\n", "unknown buffer → dropped");
    }

    #[test]
    fn availability_false_shows_notice_once() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::ClipboardAvailability(false), &mut e, &reg, &ex, &clk, &tx);
        assert!(e.status.to_lowercase().contains("clipboard"));
        assert!(e.clipboard_notice_shown);
        e.status = "typing".into();
        crate::app::reduce(Msg::ClipboardAvailability(false), &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.status, "typing", "notice shown only once");
    }
```
(`Selection::single(pos)` exists; `Selection::range(anchor, head)` is ADDED in Step 2b — these tests depend on it, so they will not compile until Step 2b lands. That is the expected RED.)

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib copy_sets_register paste_keypress clipboardpaste availability_false` → FAIL.

- [ ] **Step 2b: Add a `Selection::range` constructor** (`wordcartel-core/src/selection.rs`) — the tests need a range (anchor≠head) selection and only `Selection::single` exists (Codex plan review). Add next to `single`:
```rust
    pub fn range(anchor: BytePos, head: BytePos) -> Selection {
        Selection { ranges: smallvec::smallvec![Range { anchor, head }], primary: 0 }
    }
```
(Confirm the `smallvec` macro path / import already used in `selection.rs`; `Range`/`Selection` field names are `{anchor, head}` and `{ranges, primary}`.) This is a small IO-free core addition. Add a one-line core test: `assert_eq!(Selection::range(0,5).primary().from(), 0);`.

- [ ] **Step 3: Set intent in Copy/Cut** (`commands.rs`). In `Command::Copy`, after `register::copy(...)`, before returning, add:
```rust
            if let Some(text) = editor.register.get().map(str::to_owned) {
                editor.clipboard_sync_request = Some(text);
            }
```
In `Command::Cut`, after the register cut + apply, add the same `clipboard_sync_request = Some(text)` using the text that was cut (read `editor.register.get()` after the cut, before returning). (Cut already stored the cut text in the register via `register::cut`.)

- [ ] **Step 4: Make Paste async** (`commands.rs` `Command::Paste`). REPLACE the body's synchronous paste with:
```rust
        Command::Paste => {
            editor.clipboard_get_pending = Some(crate::clipboard::PasteIntent {
                id: crate::clipboard::next_paste_id(),
                buffer_id: editor.active().id,
            });
            CommandResult::Handled
        }
```
The previous synchronous register-paste logic moves into `insert_paste_text` (Step 5), reached via `Msg::ClipboardPaste`. **Migrate the three existing paste tests (Codex plan review named them) — `commands.rs::paste_inserts_register_at_caret` (~661), `paste_on_empty_register_is_noop` (~759), `paste_over_selection_replaces` (~919):** each currently drives `Command::Paste` and asserts an inline buffer mutation, which no longer happens. Rewrite each in `commands.rs` to assert ONLY that `Command::Paste` sets `editor.clipboard_get_pending` and leaves the buffer UNCHANGED (the command no longer inserts). The actual insert behavior they used to cover is preserved by the new `app.rs` reduce tests in Step 1 (caret insert, empty-register no-op, over-selection replace via `Msg::ClipboardPaste`). Net coverage is preserved, just relocated from the command layer to the reduce layer where the async insert now lives.

- [ ] **Step 5: Implement the shared paste helper + reduce arms** (`app.rs`):
```rust
/// The single paste-insert path: OS text, register fallback, and bracketed paste
/// all go through here. Inserts at `buffer_id`'s current cursor (CUA replace-
/// selection) as one undoable edit. Returns false (with a status) if over the cap
/// or the buffer is gone.
fn insert_paste_text(editor: &mut Editor, buffer_id: crate::editor::BufferId, text: &str, clock: &dyn Clock) -> bool {
    if text.len() > crate::clipboard::PASTE_MAX_BYTES {
        editor.status = format!("paste too large ({} MiB) — skipped", text.len() / (1 << 20));
        return false;
    }
    // Capture the active id BEFORE borrowing the target buffer, and scope the
    // by_id_mut borrow so it ends before the &mut-editor derive refresh below.
    let active_id = editor.active().id;
    {
        let Some(b) = editor.by_id_mut(buffer_id) else { return false; };
        let sel = b.document.selection.primary();
        let (from, to) = (sel.from(), sel.to());
        let doc_len = b.document.buffer.len();
        let (cs, edit) = crate::commands::build_range_replace(from, to, text, doc_len);
        let txn = wordcartel_core::history::Transaction::new(cs)
            .with_selection(wordcartel_core::selection::Selection::single(from + text.len()));
        b.apply(txn, edit, wordcartel_core::history::EditKind::Other, clock);
        b.desired_col = None; // match the existing Cut/Paste reset (Codex plan review)
    } // b borrow ends here
    if buffer_id == active_id {
        crate::derive::rebuild(editor);
        crate::nav::ensure_visible(editor);
    }
    true
}

/// Apply a clipboard paste reply (OS text or register fallback). Factored so it is
/// reachable from BOTH the normal reduce arm AND the prompt-interception block
/// (Codex plan review: an async reply must not be starved by an open modal).
/// The register is updated ONLY when the insert actually happened (not on an
/// oversize-skipped paste).
fn apply_clipboard_paste(editor: &mut Editor, buffer_id: crate::editor::BufferId, text: Option<String>, clock: &dyn Clock) {
    match text {
        Some(t) if !t.is_empty() => {
            if insert_paste_text(editor, buffer_id, &t, clock) {
                editor.register.set(t);
            }
        }
        _ => { // None / empty → register fallback (the old synchronous paste behavior)
            if let Some(t) = editor.register.get().map(str::to_owned) {
                insert_paste_text(editor, buffer_id, &t, clock);
            }
        }
    }
}

fn apply_clipboard_availability(editor: &mut Editor, ok: bool) {
    if !ok && !editor.clipboard_notice_shown {
        editor.status = "system clipboard unavailable — copy/paste work in-editor; using OSC 52 for terminal sync".into();
        editor.clipboard_notice_shown = true;
    }
}
```
Replace the temporary no-op arms (Task 2 Step 6) with real arms in BOTH `reduce`'s normal match AND the `editor.prompt.is_some()` interception block (parallel to the existing `Msg::FilterDone`/`TransformDone` arms there — the prompt block's `_ => {}` would otherwise SILENTLY DROP a paste reply / availability message; Codex plan review IMPORTANT):
```rust
        // in the normal match AND inside the `if editor.prompt.is_some()` block:
        Msg::ClipboardPaste { buffer_id, text, .. } => apply_clipboard_paste(editor, buffer_id, text, clock),
        Msg::ClipboardAvailability(ok) => apply_clipboard_availability(editor, ok),
```
(`Event::Paste` under a prompt stays ignored — it hits the prompt block's `_ => {}`, which is the desired modal behavior; only the two `Msg::Clipboard*` messages need the explicit prompt-block arms.)
Add `pub const PASTE_MAX_BYTES: usize = 8 * 1024 * 1024;` to `clipboard.rs`. The `insert_paste_text` above already resolves the borrow-split (active id captured first; the `by_id_mut` borrow scoped in a block that ends before `derive::rebuild(editor)`) — keep that structure.

- [ ] **Step 6: Run tests + suite.** `cargo test -p wordcartel --lib` then `cargo test --workspace` → all pass; `cargo build --workspace` zero warnings. Remove now-stale `#[allow(dead_code)]` on `next_paste_id`/`PasteIntent`.

- [ ] **Step 7: Commit.**
```bash
git add wordcartel/src/commands.rs wordcartel/src/app.rs wordcartel/src/clipboard.rs
git commit -m "feat(clipboard): copy/cut sync intent + async buffer-targeted paste + one-time availability notice"
```

---

### Task 4: Bracketed paste + `run()` worker/drain wiring

**Files:**
- Modify: `wordcartel/src/term.rs` (enable/disable bracketed paste), `wordcartel/src/app.rs` (`Event::Paste` arm + `run()` worker spawn + drain call)
- Test: `wordcartel/src/app.rs`

**Interfaces:**
- Consumes: `clipboard::{spawn_worker, drain_clipboard_intents}`, `insert_paste_text` (Task 3), `Minibuffer::insert`.
- Produces: bracketed paste enabled in the terminal; a per-UI-mode `Event::Paste` reduce arm; `run()` spawns the worker and drains intents each iteration.

- [ ] **Step 1: Write the failing tests** in `app.rs` (per-mode bracketed paste; no terminal needed):
```rust
    #[test]
    fn bracketed_paste_normal_inserts_into_document() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("ab\n", None, (80, 24));
        e.active_mut().document.selection = wordcartel_core::selection::Selection::single(1);
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("XY".into())), &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), "aXYb\n");
        assert_eq!(e.register.get(), Some("XY"));
        e.active_mut().undo();
        assert_eq!(e.active().document.buffer.to_string(), "ab\n");
    }

    #[test]
    fn bracketed_paste_into_minibuffer_not_document() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.open_minibuffer("> ");
        let doc_before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("cat".into())), &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.minibuffer.as_ref().unwrap().text, "cat", "paste goes into the minibuffer");
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "document untouched");
    }

    #[test]
    fn bracketed_paste_with_modal_prompt_is_ignored() {
        use crate::editor::Editor; use crate::jobs::InlineExecutor; use crate::registry::Registry;
        use crossterm::event::Event;
        let mut e = Editor::new_from_text("doc\n", None, (80, 24));
        e.prompt = Some(crate::prompt::Prompt::quit_confirm());
        let doc_before = e.active().document.buffer.to_string();
        let (tx, _rx) = std::sync::mpsc::channel();
        let reg = Registry::builtins(); let ex = InlineExecutor::default(); let clk = TestClock(0);
        crate::app::reduce(Msg::Input(Event::Paste("x".into())), &mut e, &reg, &ex, &clk, &tx);
        assert_eq!(e.active().document.buffer.to_string(), doc_before, "paste ignored under a modal");
        assert!(e.prompt.is_some());
    }
```

- [ ] **Step 2: Run to verify failure.** `cargo test -p wordcartel --lib bracketed_paste` → FAIL (no `Event::Paste` handling).

- [ ] **Step 3: Enable bracketed paste in `term.rs`.** Import `crossterm::event::{EnableBracketedPaste, DisableBracketedPaste}`. In `TerminalGuard::new()`, after the successful `execute!(stdout, EnterAlternateScreen)`, add `EnableBracketedPaste` to that `execute!` (or a follow-up `execute!(stdout, EnableBracketedPaste)`; on its error, fall through — bracketed paste is best-effort). In `Drop` add `DisableBracketedPaste` to the restore `execute!(io::stdout(), LeaveAlternateScreen, Show)` → `execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen, Show)`. Do the SAME in `install_panic_hook`'s restore. (Order: disable bracketed paste before leaving the alternate screen.)

- [ ] **Step 4: Add the `Event::Paste` reduce arm** (`app.rs`), respecting UI-mode interception order. Place it so it is reached for `Msg::Input(Event::Paste(_))` in each mode:
```rust
        Msg::Input(Event::Paste(text)) => {
            if editor.prompt.is_some() {
                // modal keypress-chooser: ignore pasted text
            } else if let Some(mb) = editor.minibuffer.as_mut() {
                for ch in text.chars() { mb.insert(ch); }
            } else {
                let bid = editor.active().id;
                if insert_paste_text(editor, bid, &text, clock) {
                    editor.register.set(text); // update register ONLY if the insert happened (not oversize-skipped)
                }
            }
        }
```
(Place this arm consistently with where `Event::Key`/`Event::Resize` are matched; the prompt/minibuffer blocks may already early-return for `Msg::Input` keys — ensure `Event::Paste` reaches THIS logic and is not swallowed as a non-key by an earlier block. If the prompt/minibuffer interception blocks only match `Event::Key`, a paste falls through to here, and this arm does the per-mode routing itself — which is what the tests pin.)

- [ ] **Step 5: Wire `run()`** (`app.rs`). Near the executor/`msg_tx` setup, add:
```rust
    let clip_tx = crate::clipboard::spawn_worker(msg_tx.clone());
```
In the loop, AFTER `let keep = reduce(...);` and BEFORE `guard.terminal().draw(...)`, add:
```rust
        crate::clipboard::drain_clipboard_intents(&mut editor, guard.terminal().backend_mut(), &clip_tx, &msg_tx);
```
(Confirm `ratatui`'s `CrosstermBackend` implements `std::io::Write` so `backend_mut()` is a valid `&mut impl Write` — it does in ratatui 0.29 by forwarding to the inner `Stdout`. If a type mismatch arises, write through `guard.terminal().backend_mut()` via its `Write` impl; do NOT open a separate `io::stdout()` handle.) The worker is detached; the existing teardown already restores the terminal before any worker concerns, so no shutdown join is added (drop `clip_tx` at end of `run()` closes the channel; the detached worker exits or is reclaimed at process end).

- [ ] **Step 6: Run tests + full suite + manual smoke.** `cargo test --workspace` → all pass; `cargo build --workspace` zero warnings. (Optional manual: run the editor, Ctrl+C a selection, paste into another app; copy in another app, Ctrl+V; terminal-paste a block — verify no stalls. Tests do not require a display server.)

- [ ] **Step 7: Commit.**
```bash
git add wordcartel/src/term.rs wordcartel/src/app.rs
git commit -m "feat(clipboard): bracketed paste (per-mode) + run() worker spawn + drain wiring"
```

---

## Self-Review (4c-3)

**Spec coverage:** §2 backend trait + dep + base64/OSC52 (Task 1); §2.1 drain seam + intent fields (Tasks 2/3); §3.1 worker off-startup init + unbounded channel + availability (Task 2); §4.1 write path copy/cut + OSC 52 ST + encoded cap (Tasks 1/3); §4.2 async buffer-targeted paste + register fallback + bracketed-paste per mode + PASTE_MAX_BYTES (Tasks 3/4); §5 one-time notice + detached-worker shutdown ordering (Tasks 3/4); §6 degrade-never-unwrap, OS-write-only-on-copy/cut (Tasks 1/3); §8 fake/seam/per-mode tests (all tasks). ✅

**Codex plan-review fixes applied (5 important + 2 minor):** (1) `Msg::ClipboardPaste`/`ClipboardAvailability` get explicit arms in the prompt-interception block too (factored into `apply_clipboard_paste`/`apply_clipboard_availability`) — the prompt block's `_ => {}` would otherwise silently drop an async reply; (2) added `Selection::range(anchor,head)` to core (only `single` existed) — Step 2b; (3) `insert_paste_text` resets `b.desired_col = None` matching the existing Cut/Paste; (4) the register is updated ONLY when the insert actually happened (oversize-skipped paste no longer mutates the register) — both the OS-text and bracketed-paste paths; (5) concrete migration of the three named existing paste tests (`paste_inserts_register_at_caret`/`paste_on_empty_register_is_noop`/`paste_over_selection_replaces`) → assert intent-set at the command layer + the insert behavior relocated to new reduce tests (caret, empty-register no-op, over-selection replace). MINOR: arboard `set_text` accepts `Into<Cow<str>>` (the `.to_owned()` is harmless); Cargo version ranges resolve to the assumed versions. Codex CONFIRMED ratatui 0.29 `CrosstermBackend: io::Write` (the run()-wiring assumption holds) and crossterm 0.28 bracketed-paste APIs.

**Codex spec-review fixes reflected:** dep stanza with `wayland-data-control`+`default-features=false` (T1); PasteIntent{id,buffer_id} buffer-targeted async paste, drop-if-closed (T3); per-mode bracketed paste (T4); detached worker + terminal-restored-first (T4 + existing order); arboard init inside the worker + unbounded channel + ClipboardAvailability (T2); OSC 52 encoded-payload cap + ST terminator + emit via backend writer between reduce and draw (T1/T4); PASTE_MAX_BYTES (T3); pure drain_clipboard_intents seam tested with Vec<u8>+fake channels (T2); OS-write-scope invariant (copy/cut only — T3); security/opt-out noted in spec §10 (no code this effort).

**Type consistency:** `ClipboardBackend`/`Arboard|Null|FakeBackend`/`osc52_set`/`OSC52_MAX_ENCODED`/`base64_encode` (T1) → `ClipReq`/`PasteIntent`/`next_paste_id`/`spawn_worker`/`drain_clipboard_intents`/`Msg::{ClipboardPaste{id,buffer_id,text},ClipboardAvailability(bool)}`/`PASTE_MAX_BYTES`/editor intent fields (T2) → Copy/Cut `clipboard_sync_request`, Paste `clipboard_get_pending`, `insert_paste_text`, the two reduce arms (T3) → `Event::Paste` arm + term.rs enable + run() `spawn_worker`/`drain_clipboard_intents` (T4). `insert_paste_text` is the single paste-insert path (OS text, register fallback, bracketed paste).

**Implementer-verify markers (real-code confirmations, not placeholders):** `arboard::Clipboard::{new,set_text,get_text}` signatures + the `wayland-data-control` feature name in the resolved 3.x; `Selection::{range,single}` constructors; `ratatui` `CrosstermBackend: io::Write` for `backend_mut()`; `crossterm::event::{EnableBracketedPaste,DisableBracketedPaste,Event::Paste}` (crossterm 0.28 — Codex-confirmed present); whether `reduce`'s prompt/minibuffer interception blocks match only `Event::Key` (so `Event::Paste` falls through to the new arm). Each names what to check and where.

---

## Execution Handoff

Plan complete. Recommended: **subagent-driven execution** (fresh subagent per task + per-task review), then an opus whole-branch review and a Codex pre-merge gate before merge — the flow that shipped 4c-1 and 4c-2.
