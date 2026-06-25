# Wordcartel Effort 4c-3 — System Clipboard Sync (design)

**Status:** design (brainstormed + Codex-reviewed 2026-06-24)
**Sibling efforts:** 4c-1 filter primitive + pandoc export (✅ merged); 4c-2 repar transforms (✅ merged).
**Spec source:** main design §3.6 (selection + system clipboard), §9.5 (clipboard/terminal gotchas), §15.6 (clipboard & terminal capabilities), §11 (fake-backed testing); coverage ledger row 4c.

---

## 1. Goal

Sync Wordcartel's existing internal copy/cut/paste **register** to and from the **OS clipboard**, so text copied in the editor is pastable in other apps and vice-versa — over a local terminal **and over SSH**. The sync uses `arboard` (X11 + Wayland) with an **OSC 52** terminal fallback, plus **bracketed paste** for the universal terminal paste path.

**Core principle (§9.5/§15.6 — binding):** the in-process `Register` (`wordcartel-core::register`) is ALWAYS the source of truth; copy/cut/paste already work entirely through it. The OS clipboard is a **degradable sync layer** that must **never block the main loop and never break editing**. An unavailable system clipboard is a no-op *for OS sync only*. Never `unwrap` a clipboard op.

## 2. Architecture

Functional-core/imperative-shell holds: clipboard I/O (arboard, terminal escape sequences, the worker thread) lives in the **shell crate** (`wordcartel`); `wordcartel-core` is untouched. The existing `Register` and the `Command::{Copy,Cut,Paste}` semantics in `wordcartel/src/commands.rs` stay the source of truth; this effort adds an OS-sync layer around them.

- **New file:** `wordcartel/src/clipboard.rs` — the `ClipboardBackend` trait + impls, the worker thread, the OSC 52 encoder, and the pure `drain_clipboard_intents` seam.
- **New dependency (exact stanza — Codex CRITICAL/MINOR):**
  ```toml
  arboard = { version = "3", default-features = false, features = ["wayland-data-control"] }
  ```
  in `wordcartel/Cargo.toml` (shell crate only; NOT `wordcartel-core`). `default-features = false` drops `image-data` (we are text-only); `wayland-data-control` is REQUIRED for native-Wayland clipboard. **Backend selection (verified, arboard 3.6.0 `platform/linux/mod.rs:91`):** with the feature on and `WAYLAND_DISPLAY` set, arboard initializes a native Wayland clipboard via `wl-clipboard-rs` using the **`wlr-data-control`** protocol (the `wl-copy`/`wl-paste` mechanism); if that fails it **falls back to X11** (via Xwayland). So wlroots compositors (Hyprland/Sway/river) get native Wayland; GNOME/older-KDE (no `wlr-data-control`) fall back to X11 over Xwayland; only a session with neither degrades to `NullBackend` (still covered by OSC 52 + bracketed paste). (Confirm the exact arboard 3.x version + feature name at implementation time.)
- **No new base64 dependency:** OSC 52 needs base64; a small self-contained encoder lives in `clipboard.rs` (≈15 lines, unit-tested).
- **Reused:** the unified `Msg`/`reduce` loop, `Ctx.msg_tx`, the existing `Register`, `Command::Paste`, `build_range_replace`/`replace_changeset`, `Buffer::apply` (`EditKind::Other`), `editor::by_id_mut`, the terminal setup/teardown + panic guard in `run()` and `term.rs`.

### 2.1 The shell-wiring constraint (commands set intent; the loop fulfills)

The `Command::{Copy,Cut,Paste}` handlers run inside `reduce` with a `&mut Editor` but **not** the terminal output (the render loop owns it) nor the worker channel. So a command sets **intent fields** on `Editor`, and a single **pure, injectable drain function** — called by the `run()` loop after each `reduce` — fulfills them:

```rust
// In clipboard.rs — testable with a Vec<u8> writer + fake channels (Codex IMPORTANT).
pub fn drain_clipboard_intents(
    editor: &mut Editor,
    out: &mut impl std::io::Write,                 // the terminal backend writer (or a Vec<u8> in tests)
    clip_tx: &std::sync::mpsc::Sender<ClipReq>,    // unbounded → send never blocks
    msg_tx: &std::sync::mpsc::Sender<Msg>,
);
```
- `Editor.clipboard_sync_request: Option<String>` — set by Copy/Cut. Drain: write OSC 52 bytes to `out`, send `ClipReq::Set(text)`, clear the field.
- `Editor.clipboard_get_pending: Option<PasteIntent>` — set by Paste (see §4.2). Drain: send `ClipReq::Get { id, buffer_id }` to the worker, or — if there is no worker — push `Msg::ClipboardPaste { id, buffer_id, text: None }` to `msg_tx` so the register fallback fires; clear the field.

`drain_clipboard_intents` is the integration seam the tests exercise (concrete `run()` stays a thin caller). The OSC 52 bytes go through the **same writer the renderer flushes** (`terminal.backend_mut()`), emitted at a fixed point **after `reduce`, before the frame draw**, so they never interleave mid-frame (Codex IMPORTANT — no separate `io::stdout()` handle).

## 3. The backend abstraction (degradation + testability)

```rust
pub trait ClipboardBackend: Send {
    fn set(&mut self, text: &str);        // best-effort; errors swallowed
    fn get(&mut self) -> Option<String>;  // None on empty/error/unavailable
}
```
- **`ArboardBackend`** — holds a **long-lived** `arboard::Clipboard` (required on X11 to retain selection ownership while the editor runs; arboard manages its own internal X11 owner thread for `set_text`).
- **`NullBackend`** — arboard init failed / no display: `set` no-op, `get` → `None`. (OSC 52 still covers the SSH/terminal direction.)
- **`FakeBackend`** — in-memory `Option<String>` for tests (no real display server).

### 3.1 The clipboard worker thread (owns the backend; inits arboard OFF the startup path)

A single long-lived worker thread **owns the backend** and serves an **unbounded** mpsc channel:
```rust
pub enum ClipReq {
    Set(String),
    Get { id: u64, buffer_id: BufferId },
    Shutdown,
}
```
- **Init happens inside the worker, not on the main thread** (Codex IMPORTANT): the worker calls `arboard::Clipboard::new()` after it starts. On success → `ArboardBackend`; on failure → `NullBackend`. It reports availability back exactly once via `Msg::ClipboardAvailability(bool)` (drives the §5 one-time notice). This keeps `arboard::Clipboard::new()`'s potential X-connect stall entirely off the editor's startup path — the editor is responsive immediately.
- `ClipReq::Set(s)` → `backend.set(&s)` (fire-and-forget; retains ownership).
- `ClipReq::Get { id, buffer_id }` → `backend.get()` → `msg_tx.send(Msg::ClipboardPaste { id, buffer_id, text })`.
- `ClipReq::Shutdown` → break the loop and drop the backend (see §5 shutdown ordering).

The channel is **unbounded** so the main-loop `send` never blocks even if the worker is mid-`get`. A pathological X hang on `get` stalls *subsequent clipboard ops* but **never the editor main loop**; a per-`Get` timeout is a documented deferred refinement.

## 4. Data flow

### 4.1 Write path (copy / cut) — register inline, OS sync off-thread
In the existing `Command::Copy`/`Cut` handlers, AFTER setting the internal register (unchanged — instant "Copied"/cut), set `editor.clipboard_sync_request = Some(text)`. `drain_clipboard_intents` then:
1. **OSC 52** — writes `\x1b]52;c;<base64(text)>\x1b\\` to the terminal writer. **Terminator = ST (`ESC \\`)**, which tmux/screen/most modern terminals accept more reliably than BEL; a BEL variant is a deferred config knob. **Size cap is on the ENCODED payload** (Codex IMPORTANT): skip OSC 52 when `base64(text).len() > OSC52_MAX_ENCODED` (= 100_000 bytes ≈ 74 KiB raw); arboard still local-syncs larger text. (base64 expands ~4/3, so the cap is defined on the post-encode length, not the raw text.)
2. **Worker `Set`** — sends `ClipReq::Set(text)`.
Both are independent best-effort: arboard covers local, OSC 52 covers SSH; either degrading is fine. The register (in-editor paste) is already done regardless.

### 4.2 Read path (paste) — async, buffer-targeted, never blocks

**Paste intent (Codex IMPORTANT — async paste must be tied to the buffer that requested it):**
```rust
pub struct PasteIntent { pub id: u64, pub buffer_id: BufferId } // id from a monotonic counter
```
- **`Ctrl+V` (`Command::Paste`)** sets `editor.clipboard_get_pending = Some(PasteIntent { id, buffer_id: editor.active().id })` and returns WITHOUT pasting. The drain sends `ClipReq::Get { id, buffer_id }`; the worker replies `Msg::ClipboardPaste { id, buffer_id, text }`. The `reduce` arm:
  - resolve the target buffer via `by_id_mut(buffer_id)`; if it no longer exists (future multi-buffer close) → **drop** the paste (no-op).
  - `text = Some(s)` non-empty → `register.set(s)` then insert `s` at **that buffer's current cursor** (CUA replace-selection if a selection is active) as one undoable `EditKind::Other` edit.
  - `text = None` / empty → **register fallback**: insert `register.get()` at that buffer's cursor (the existing paste logic, factored into a shared helper so today's Paste tests still cover it).
  - Concurrent `Ctrl+V` presses each carry a distinct `id` and apply independently in arrival order (each one undoable edit) — no double-paste, no lost paste. (`id` is reserved for a future "only honor the latest" policy; v1 applies all.)
  One undoable edit per paste; one frame of latency locally; the loop never blocks.
- **Bracketed paste (`Event::Paste(text)`)** — `EnableBracketedPaste` at terminal setup; `DisableBracketedPaste` at teardown AND in the panic-guard restore (mirroring how the existing terminal enhancements are toggled in `term.rs`). A new `reduce` arm handles `Msg::Input(Event::Paste(text))` **respecting UI mode (Codex IMPORTANT), in the same interception order as key input:**
  - **modal prompt open** (`editor.prompt.is_some()`) → **ignore** the paste (a keypress-chooser takes no text).
  - **minibuffer open** (`editor.minibuffer.is_some()`) → insert the text into the **minibuffer input** at its cursor (consistent with how typed printables route to the minibuffer).
  - **normal mode** → insert `text` at the document cursor as one undoable edit and update the register.

**Large-paste policy (Codex IMPORTANT — a multi-MB paste must not freeze the loop):** both `Event::Paste(text)` and `Msg::ClipboardPaste` insert inline (a single ropey edit). A hard cap `PASTE_MAX_BYTES` (= 8 MiB) **rejects** an over-cap paste with a status (`"paste too large (N MiB) — skipped"`) rather than building a huge ChangeSet on the loop. Below the cap, the insert is one bounded operation (documented: a large-but-accepted paste may cost a brief, bounded stall — a single insert, not N keystrokes). Routing above-threshold pastes through an async job is a deferred refinement (§9).

## 5. Degradation, one-time notice & shutdown ordering

- **Availability + one-time notice:** the worker reports `Msg::ClipboardAvailability(false)` when arboard init fails; `reduce` shows a **one-time** status: `"system clipboard unavailable — copy/paste work in-editor; using OSC 52 for terminal sync"`. Shown once (a `notice_shown` latch); never repeated. Editing is never affected.
- **Shutdown ordering (Codex IMPORTANT — avoid the 4b-1 terminal-restore hostage):** on quit, the `run()` loop **restores the terminal FIRST** (leave alternate screen, disable bracketed paste + enhancements, show cursor — the existing teardown/guard order), THEN signals the clipboard worker via `ClipReq::Shutdown` (or by dropping `clip_tx`, closing the channel). The worker is **detached** (not joined on the hot exit path) so a worker blocked in arboard `get`, or a slow arboard `Drop` (which joins arboard's own X11 server thread), can NEVER hold terminal restoration or process exit hostage. The OS reclaims the detached thread at process end. (This mirrors the 4b-1 fix: terminal restored before any worker teardown.)

## 6. Error handling & edge cases

(Per §15: degrade, don't abort; never `unwrap`; never lose work.)
- **arboard error** (set or get, or init) → swallowed: set no-op, get `None`, init → `NullBackend`. Never surfaced as an error, never panics.
- **Empty OS clipboard** on `Ctrl+V` → `None` → register fallback.
- **OSC 52 over the encoded cap** → OSC 52 skipped (arboard still local-syncs); no error.
- **Paste targets a closed buffer** (future multi-buffer) → dropped (no-op).
- **Paste over the size cap** → rejected with a status; buffer untouched.
- **Copy of an empty selection** → existing guard holds (no register overwrite); no sync fired.
- **Paste with an active selection** → CUA replace-selection preserved (only the source text differs).
- **No worker (headless tests)** → `Command::Paste`'s intent resolves to `Msg::ClipboardPaste { text: None, .. }` → register fallback; `clipboard_sync_request` is dropped (OSC bytes still written to the test writer).
- **OS write scope (Codex MINOR):** OS clipboard writes (OSC 52 + `ClipReq::Set`) happen **only on explicit Copy/Cut**. Paste paths (OS read, bracketed paste) may update the in-process register but **never re-emit to the OS** — no feedback loop.

## 7. Components / module boundaries

| Unit | Responsibility | Depends on |
|------|----------------|------------|
| `clipboard.rs::ClipboardBackend` (+ Arboard/Null/Fake) | OS get/set, degradable | `arboard` |
| `clipboard.rs::spawn_worker(msg_tx) -> Sender<ClipReq>` | own backend, init arboard off-startup, serve Set/Get/Shutdown, report availability | `Msg`, backend |
| `clipboard.rs::osc52_set(text) -> Option<Vec<u8>>` + base64 | encode OSC 52 (None if over cap) | — |
| `clipboard.rs::drain_clipboard_intents(editor, out, clip_tx, msg_tx)` | pure seam: intent fields → OSC bytes + ClipReq | `Editor`, channels |
| `commands.rs` Copy/Cut/Paste | set register + intent fields | `Register`, `Editor` |
| `app.rs::run()` | call drain after reduce (via terminal backend writer); enable bracketed paste; shutdown ordering | terminal, worker |
| `app.rs::reduce` arms | `Msg::ClipboardPaste`/`ClipboardAvailability`; `Event::Paste` per-mode | `build_range_replace`, `Register`, `by_id_mut` |
| `editor.rs` fields | `clipboard_sync_request: Option<String>`, `clipboard_get_pending: Option<PasteIntent>`, `clipboard_notice_shown: bool` | — |

## 8. Testing (fakes, per §11 — no real display/terminal)

- **`FakeBackend`** set/get round-trip.
- **OSC 52 encoding** — `osc52_set("hi")` → exactly `\x1b]52;c;aGk=\x1b\\`; over-cap input → `None` (skip). Base64 unit tests for known vectors.
- **`drain_clipboard_intents`** (the integration seam) — with a `Vec<u8>` writer + fake channels: a `clipboard_sync_request` produces the OSC bytes on the writer AND a `ClipReq::Set` on the channel; a `clipboard_get_pending` produces a `ClipReq::Get { id, buffer_id }` (or, with a closed channel/no worker, a `Msg::ClipboardPaste{text:None}`).
- **Async paste** — `reduce(Msg::ClipboardPaste { text: Some("X"), buffer_id, id })` sets the register and inserts "X" at that buffer's cursor (one undo restores); `text: None` → register fallback; a `buffer_id` that no longer exists → no-op; two pastes with distinct ids both apply.
- **Bracketed paste per mode** — `Event::Paste("t")`: normal → document insert (one undo restores) + register updated; minibuffer open → goes into the minibuffer line, document untouched; modal prompt open → ignored.
- **Size cap** — a `> PASTE_MAX_BYTES` paste → rejected with status, buffer untouched.
- **Degradation + notice** — `Msg::ClipboardAvailability(false)` shows the notice once (second one is a no-op); `NullBackend.get()` → `None`.
- No prior test weakened; `cargo build --workspace` zero warnings; functional-core untouched.

## 9. Non-goals (explicit)

- **PRIMARY selection / X11 middle-click** (auto-copy on select, middle-click paste) — needs selection-change + mouse-event wiring, X11-only; **deferred to a later effort**.
- **Async-job routing of large pastes** (above-threshold paste off the loop with a "pasting…" indicator, §9.5) — v1 inserts inline below `PASTE_MAX_BYTES` and rejects above it; the async path is a later refinement.
- **Clipboard-read timeout / hung-worker recovery** — documented limitation; a per-`Get` timeout is a later refinement.
- **Native-Wayland clipboard without the `wayland-data-control` feature / without Xwayland** — covered by the feature flag; if a target lacks both, it degrades to `NullBackend` + OSC 52 (acceptable, noted).
- **Multiple named registers / clipboard history** — out of scope.
- **BEL-vs-ST OSC terminator config, per-Get timeout, OSC 52 opt-out kill switch** — see §10; simple future knobs, not v1 blockers.

## 10. Security note (OSC 52)

OSC 52 transmits copied text as a (base64) escape sequence **through the terminal**, which over SSH/tmux/screen sets the **local** machine's clipboard. This is the intended SSH benefit, but it means copied content crosses the terminal boundary. The size cap bounds exposure; a future **opt-out** (`WORDCARTEL_NO_OSC52` env var or config key) to disable OSC 52 emission while keeping arboard is a small, recommended follow-up (noted, not a v1 blocker). arboard-only (no OSC 52) remains fully functional locally.
