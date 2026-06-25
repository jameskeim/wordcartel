# Wordcartel Effort 4c-3 — System Clipboard Sync (design)

**Status:** design (brainstormed 2026-06-24)
**Sibling efforts:** 4c-1 filter primitive + pandoc export (✅ merged); 4c-2 repar transforms (✅ merged).
**Spec source:** main design §3.6 (selection + system clipboard), §9.5 (clipboard/terminal gotchas), §15.6 (clipboard & terminal capabilities), §11 (fake-backed testing); coverage ledger row 4c.

---

## 1. Goal

Sync Wordcartel's existing internal copy/cut/paste **register** to and from the **OS clipboard**, so text copied in the editor is pastable in other apps and vice-versa — over a local terminal **and over SSH**. The sync uses `arboard` (X11 + Wayland) with an **OSC 52** terminal fallback, plus **bracketed paste** for the universal terminal paste path.

**Core principle (§9.5/§15.6 — binding):** the in-process `Register` (`wordcartel-core::register`) is ALWAYS the source of truth; copy/cut/paste already work entirely through it. The OS clipboard is a **degradable sync layer** that must **never block the main loop and never break editing**. An unavailable system clipboard is a no-op *for OS sync only*. Never `unwrap` a clipboard op.

## 2. Architecture

Functional-core/imperative-shell holds: clipboard I/O (arboard, terminal escape sequences, the worker thread) lives in the **shell crate** (`wordcartel`); `wordcartel-core` is untouched. The existing `Register` and the `Command::{Copy,Cut,Paste}` semantics in `wordcartel/src/commands.rs` stay the source of truth; this effort adds an OS-sync layer around them.

- **New file:** `wordcartel/src/clipboard.rs` — the `ClipboardBackend` trait, its implementations, the worker thread, and the OSC 52 encoder.
- **New dependency:** `arboard` in `wordcartel/Cargo.toml` (shell crate only; NOT `wordcartel-core`).
- **No new base64 dependency:** OSC 52 needs base64; a small self-contained encoder lives in `clipboard.rs` (≈15 lines, unit-tested). (A `base64` crate is an acceptable alternative; default is the in-house encoder to keep the dependency surface minimal.)
- **Reused:** the unified `Msg`/`reduce` loop, `Ctx.msg_tx`, the existing `Register`, `Command::Paste`, `build_range_replace`/`replace_changeset`, `Buffer::apply` (`EditKind::Other`), the terminal setup/teardown + panic guard in `run()`.

### 2.1 The shell-wiring constraint (why commands set "intent fields")

The `Command::{Copy,Cut,Paste}` handlers run inside `reduce` and have a `&mut Editor` but **not** the terminal output (the render loop owns stdout) nor the clipboard worker's request channel. So a command cannot itself emit OSC 52 or talk to the worker. Instead, commands set **intent fields** on `Editor`, and the `run()` loop — which holds the terminal writer and the worker channel — drains them after each `reduce`:

- `Editor.clipboard_sync_request: Option<String>` — set by Copy/Cut to the text just copied; the loop emits OSC 52 + sends it to the worker, then clears it.
- `Editor.clipboard_get_pending: bool` — set by Paste; the loop sends a `Get` request (or, with no worker, synthesizes `Msg::ClipboardPaste(None)`), then clears it.

This keeps `commands.rs` free of terminal/thread concerns and keeps the whole flow testable (a test asserts the intent field without a real loop/worker).

## 3. The backend abstraction (degradation + testability)

```rust
pub trait ClipboardBackend: Send {
    fn set(&mut self, text: &str);        // best-effort; errors swallowed
    fn get(&mut self) -> Option<String>;  // None on empty/error/unavailable
}
```
- **`ArboardBackend`** — holds a **long-lived** `arboard::Clipboard` (required on X11 to retain ownership while the editor runs). `set`/`get` map to arboard, swallowing errors (→ no-op / `None`).
- **`NullBackend`** — no display server / arboard init failed: `set` is a no-op, `get` returns `None`. (OSC 52 still covers the SSH/terminal direction.)
- **`FakeBackend`** — in-memory `Option<String>` for tests (no real display server needed).

### 3.1 The clipboard worker thread
A single long-lived thread **owns the backend** and serves requests over an mpsc channel:
```rust
enum ClipReq { Set(String), Get }
```
- `ClipReq::Set(s)` → `backend.set(&s)` (fire-and-forget; retains X11 ownership).
- `ClipReq::Get` → `backend.get()` → send `Msg::ClipboardPaste(result)` back via `msg_tx`.

One worker (not per-op) because arboard must keep its handle alive to retain ownership. A pathological X-server hang on a `Get` blocks *subsequent clipboard ops* but **never the editor main loop** (which only ever sends on the channel). A read timeout / hung-worker recovery is a documented **known limitation**, deferred.

## 4. Data flow

### 4.1 Write path (copy / cut) — register inline, OS sync off-thread
In the existing `Command::Copy`/`Cut` handlers, AFTER setting the internal register (unchanged — instant "Copied"/cut), set `editor.clipboard_sync_request = Some(text)`. The `run()` loop then, after `reduce`:
1. **OSC 52 inline** — writes `\x1b]52;c;<base64(text)>\x07` to the terminal output (reaches the *local* terminal even over SSH; harmless locally). **Skipped when the text exceeds `OSC52_MAX_BYTES` (= 64 KiB)** — most terminals reject larger OSC payloads; arboard still syncs locally.
2. **Worker `Set`** — sends `ClipReq::Set(text)` to the worker (arboard, off-thread).
Both are independent best-effort: arboard covers local, OSC 52 covers SSH; either degrading is fine. The register (and thus in-editor paste) is already done regardless.

### 4.2 Read path (paste) — async, never blocks
- **`Ctrl+V` (`Command::Paste`)** sets `editor.clipboard_get_pending = true` and returns without pasting immediately. The `run()` loop sends `ClipReq::Get` to the worker (or, if there is no worker, sends itself `Msg::ClipboardPaste(None)`). The worker replies with `Msg::ClipboardPaste(Option<String>)`. The `reduce` arm for it:
  - `Some(text)` non-empty → `editor.register.set(text.clone())` then **insert at the current cursor** (re-read selection at apply time; CUA replace-selection if a selection is active) as one undoable `EditKind::Other` edit.
  - `None` / empty → **fall back to the existing register paste** (insert `register.get()` at the cursor) — identical to today's behavior.
  Either branch yields exactly one undoable edit. Latency is one frame locally (imperceptible); the loop never blocks on the clipboard.
- **Bracketed paste (`Event::Paste(text)`)** — `EnableBracketedPaste` is issued at terminal setup in `run()` (and `DisableBracketedPaste` at teardown + in the panic guard). A new `reduce` arm for `Msg::Input(Event::Paste(text))` inserts `text` directly at the cursor as one undoable edit and updates the register. This is the universal terminal OS→editor path and the reliable way to paste **external** content over SSH (where arboard `get` is unavailable).

## 5. Degradation & one-time notice

At `run()` startup, try `arboard::Clipboard::new()`:
- success → `ArboardBackend` in the worker.
- failure (headless, no display, Wayland-without-protocol) → `NullBackend` and a **one-time** status notice: `"system clipboard unavailable — copy/paste work in-editor; using OSC 52 for terminal sync"`. Shown once; not repeated.
Editing is never affected; the register always works; OSC 52 still operates regardless of arboard.

## 6. Error handling & edge cases

(Per §15: degrade, don't abort; never `unwrap`; never lose work.)
- **arboard error** (set or get) → swallowed: set is a no-op, get is `None`. Never surfaced as an error, never panics.
- **Empty OS clipboard** on `Ctrl+V` → `None` → register fallback.
- **OSC 52 over `OSC52_MAX_BYTES`** → OSC 52 skipped (arboard still local-syncs); no error.
- **Copy of an empty selection** → the existing guard (no register overwrite) holds; no sync fired.
- **Paste with an active selection** → CUA replace-selection semantics preserved (only the source text differs).
- **No worker (headless tests)** → `Command::Paste` resolves to `Msg::ClipboardPaste(None)` → register fallback; `clipboard_sync_request` is simply dropped.
- **Hung clipboard worker** (pathological X hang on `get`) → future clipboard ops stall but the editor never blocks; documented limitation (timeout deferred).

## 7. Components / module boundaries

| Unit | Responsibility | Depends on |
|------|----------------|------------|
| `clipboard.rs::ClipboardBackend` (+ Arboard/Null/Fake impls) | OS clipboard get/set, degradable | `arboard` |
| `clipboard.rs::worker` (thread + `ClipReq`) | own the backend, serve Set/Get, reply via `msg_tx` | `Msg`, `ClipboardBackend` |
| `clipboard.rs::osc52_set(text) -> Vec<u8>` + base64 | encode the OSC 52 escape sequence | — |
| `commands.rs` Copy/Cut/Paste | set register + intent fields (`clipboard_sync_request`/`clipboard_get_pending`) | `Register`, `Editor` |
| `app.rs::run()` loop drain | emit OSC 52 + send `ClipReq` from intent fields; enable bracketed paste | terminal, worker channel |
| `app.rs::reduce` arms | `Msg::ClipboardPaste(opt)` insert/fallback; `Event::Paste(text)` insert | `build_range_replace`, `Register` |
| `editor.rs` fields | `clipboard_sync_request: Option<String>`, `clipboard_get_pending: bool` | — |

## 8. Testing (fakes, per §11)

- **`FakeBackend`** set/get round-trip (no real display server).
- **OSC 52 encoding** — `osc52_set("hi")` produces the exact `\x1b]52;c;aGk=\x07` byte sequence (assert base64 + framing for a known string); over-size input → empty/skip.
- **Async paste flow** — `reduce(Msg::ClipboardPaste(Some("X")))` sets the register and inserts "X" at the cursor as one undoable edit (one undo restores the original); `reduce(Msg::ClipboardPaste(None))` falls back to the register paste. `Command::Paste` sets `clipboard_get_pending`.
- **Bracketed paste** — `reduce(Msg::Input(Event::Paste("text")))` inserts at the cursor (one undo restores) and updates the register.
- **Write intent** — `Command::Copy`/`Cut` set `clipboard_sync_request` to the copied text (and the register), without touching the terminal.
- **Degradation** — `NullBackend.get()` → `None` → register used; the one-time notice fires once.
- No prior test weakened; `cargo build --workspace` zero warnings; functional-core untouched.

## 9. Non-goals (explicit)

- **PRIMARY selection / X11 middle-click** (auto-copy on select, middle-click paste) — needs selection-change + mouse-event wiring, X11-only; **deferred to a later effort**.
- **Off-loop threading of huge pastes** (§9.5 "pasting…" indicator) — v1 inserts bracketed/clipboard pastes inline (a single ropey insert is fast); thread later if a real perf issue appears.
- **Clipboard-read timeout / hung-worker recovery** — documented limitation; a per-`Get` timeout is a later refinement.
- **Multiple named registers / clipboard history** — out of scope.
