# Effort A — harper-ls diagnostics provider: embedded Harper → external LSP — design spec

**Date:** 2026-07-11
**Status:** spec for review (Codex gate)
**Verified against:** `main` @ `53d2ae6`. All symbol names below were grep-verified against this
tree; anchors are symbol names, never line numbers.
**Approved design inputs:** the Effort-A Phase-0 surface map + brainstorm decisions ledger (all
locked decisions and fork resolutions are baked in below, not re-opened).

---

## 0. Summary and the product change

Wordcartel today embeds `harper-core = "2"` in `wordcartel-core` and runs grammar/spell checks
in-process: `diagnostics_run::dispatch_diagnostics` spawns a thread per check that calls the pure
`wordcartel_core::diagnostics::check` and sends `Msg::DiagnosticsDone`. That one dependency pulls
the entire `burn` tensor stack transitively into our build (H2), and the first check pays an ~11 s
dictionary warm-up that `app.rs` hides behind a startup warm thread.

Effort A swaps the backend for the **external `harper-ls` language server** behind a thin,
mockable **`DiagnosticsProvider` seam**:

- `harper-core` is **removed from `wordcartel-core` entirely** (locked F6 — no cargo-feature dual
  path). The pure data types `Diagnostic` / `DiagnosticKind` / `Suggestion` stay in core; the
  Harper invocation (`check`, `CheckOpts`, the adapter) is deleted.
- A new shell-side LSP client (imperative shell — process IO; locked F5) spawns `harper-ls`
  **lazily on entering Review**, keeps **one shared server for N buffers** (URIs; `untitled:` for
  unsaved), speaks a **hand-rolled JSON-RPC/stdio subset over `lsp-types`** on the existing
  thread + mpsc substrate, and pushes results as the **existing `Msg::DiagnosticsDone`** — so
  `DiagStore`, `apply_diagnostics_done`, the overlay, and `diag_apply_selected` keep their shapes.
- harper-ls fixes are codeAction-only, so the client **eager-assembles**: on
  `publishDiagnostics` it batches one `textDocument/codeAction` request over the diagnostics'
  span, maps the returned `TextEdit`s onto `Diagnostic.suggestions`, and only then emits a
  complete `Vec<Diagnostic>` — preserving today's pre-attached-suggestions overlay model
  (locked F2). §5 makes the version-staleness correctness argument explicit.
- Degradation (locked F1): harper-ls absent → Review still renders, the status line shows an
  install hint, the editor is fully functional. Arch packaging gains `optdepends=harper`.
- H18 folds in as the effort tail: a `cargo deny` supply-chain configuration against the
  post-swap dependency tree.

The user-visible surface is unchanged except: (a) grammar checking now requires `harper-ls` on
PATH (deliberate — grammar becomes opt-in, fitting E7's "deliberate Review" identity), (b) the
status line reads `[REVIEW · Harper]` when the provider is live (minimal multi-provider
attribution, locked scope nit), and (c) the "grammar on" lint surface follows harper-ls's curated
defaults, which is somewhat richer than the old four-`LintKind` filter (§7.2 states this delta
explicitly).

---

## 1. Architecture overview

```
                 ┌───────────────────────────── main loop (run/reduce) ─────────────────────────────┐
 set_render_mode │ arm(now,0)                                                                       │
 (enter Review)  │   ↓ timers::next_wake / on_tick                                                  │
                 │ diagnostics_run::dispatch_diagnostics(editor)                                    │
                 │   ├─ consumes recheck_due_at, sets in_flight_version                             │
                 │   └─ editor.diag_provider.notify_change(id, version, path, text) ──┐             │
                 │                                                                    │ mpsc Cmd    │
                 │ Msg::DiagnosticsDone ← version-gated apply_diagnostics_done        │             │
                 │ Msg::DiagProviderEvent ← status/lifecycle                          │             │
                 └────────────────────────────────────────────────────────────────────┼─────────────┘
                                                                                      ↓
                 ┌── wcartel-harper-client thread (owns Child + stdin, HarperState machine) ──┐
                 │ initialize/initialized → didChangeConfiguration → didOpen/didChange (FULL) │
                 │ publishDiagnostics → eager-assemble (codeAction batch) → DiagnosticsDone   │
                 │ crash → bounded respawn → degrade                                          │
                 └──────────────┬───────────────────────────────────────────────────────────-─┘
                                │ framed stdio (Content-Length JSON-RPC)
                 ┌── wcartel-harper-read thread (stdout framing reader) ──┐      harper-ls child
```

Three new shell modules (all in `wordcartel/src/`):

| module | responsibility |
|---|---|
| `diag_provider.rs` | the `DiagnosticsProvider` trait, `Availability`, `ProviderEvent`, `NullProvider`, `apply_provider_event`, the session-ignore predicate |
| `lsp_rpc.rs` | pure/IO-light plumbing: Content-Length framing (read + write), JSON-RPC envelope encode/decode over `serde_json::Value` + `lsp-types`, UTF-16→byte position conversion, `TextEdit`→`Suggestion` mapping, Spelling/Grammar classification |
| `harper_ls.rs` | the `HarperLs` provider: app-side handle, client thread + `FlushGuard`, child spawn/respawn/shutdown, the `HarperState` protocol state machine (pure, unit-testable) incl. the `workspace/configuration` PULL responder, eager-assembly |

No module is a dispatch hub; none needs a `module_budgets.rs` row. Functions stay under the
100-line `clippy::too_many_lines` threshold by construction (the `HarperState` event handler is a
`match` over a small enum whose arms delegate to per-message methods).

New shell dependencies: `lsp-types = "0.97"` (typed params where convenient) and
`serde_json = "1"` (envelopes as `Value`; already transitively in `Cargo.lock`). No `url` crate —
URIs are opaque `untitled:`-scheme strings built with `format!` (§3.3), so there is no `file://`
construction. No tower-lsp / lsp-server / tokio — the client is a hand-rolled subset on
`std::thread` + `mpsc`, matching the house job substrate. The child is spawned with
`std::process::Command` (piped stdio; the existing `subprocess` crate dep is for the filter/export
paths and is not needed here — we manage framing and lifetime ourselves).

---

## 2. The `DiagnosticsProvider` seam (`diag_provider.rs`)

Thin by design (locked): lifecycle + configure + notify + emit. No merge/multi-provider
machinery — harper is the only provider; the seam is Open–Closed insurance and gives provider #2
a socket, nothing more.

```rust
/// Where diagnostics come from. Implementations own their transport; results are emitted
/// asynchronously as `Msg::DiagnosticsDone` (and lifecycle changes as `Msg::DiagProviderEvent`)
/// on the `Sender<Msg>` the implementation was constructed with. All methods are non-blocking:
/// they enqueue work for a provider-owned thread and return immediately (hot-path law).
pub trait DiagnosticsProvider: std::fmt::Debug {
    /// Short display name for status attribution ("Harper").
    fn name(&self) -> &'static str;
    /// Current lifecycle state (cheap; readable every frame by render).
    fn availability(&self) -> Availability;
    /// Idempotent lazy start: first call spawns the client thread (which spawns the child and
    /// runs the init handshake); later calls are no-ops. Never blocks on the handshake.
    fn ensure_running(&mut self);
    /// Push provider configuration (dictionary path, grammar partition, limits). Called once at
    /// install; a running provider re-sends `workspace/didChangeConfiguration`.
    fn configure(&mut self, cfg: ProviderConfig);
    /// Full-document text sync for one buffer at one version. Returns whether the change was
    /// **accepted** (successfully enqueued to a live client thread). The provider guarantees:
    /// **accepted ⟹ at least one terminal `Msg::DiagnosticsDone` for this `(buffer_id, version)`
    /// will arrive** (a real publish, or an empty version-tagged set from a watchdog / crash-flush
    /// / thread-exit-flush). `Accepted::No` ⟹ the provider emitted nothing and the caller must NOT
    /// set the in-flight latch (the wedge fix, §5). See `dispatch_diagnostics` (§4.3).
    fn notify_change(&mut self, buffer_id: BufferId, version: u64,
        path: Option<std::path::PathBuf>, text: String) -> Accepted;
    /// The buffer is gone; release server-side state (didClose).
    fn notify_close(&mut self, buffer_id: BufferId);
    /// Best-effort: ask the server to re-read `userDictPath` (harper re-reads on a
    /// `didChangeConfiguration` resend). **Not a writer** — our `append_word_to_dict` is the sole
    /// dictionary-file writer (§7.4); this only nudges the server's own copy so it stops emitting
    /// the already-client-suppressed lint. Never blocks; failure is immaterial (the client filter
    /// hides dictionary words regardless).
    fn reload_dictionary(&mut self);
    /// Begin clean shutdown (LSP shutdown/exit + bounded child kill). Non-blocking.
    fn shutdown(&mut self);
}

/// notify_change acceptance — a two-state result so the caller sets the in-flight latch only when
/// a terminal DiagnosticsDone is guaranteed (§5 latch invariant).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Accepted { Yes, No }

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Availability {
    /// ensure_running has not been called yet (pre-first-Review), or NullProvider.
    Idle,
    /// Client thread is up; init handshake not yet complete. Commands queue.
    Starting,
    /// Initialized; serving.
    Ready,
    /// Not installed, or crashed past the respawn budget. Terminal for the session.
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub grammar: bool,
    pub dictionary: Option<std::path::PathBuf>, // → harper-ls userDictPath (omitted when None)
    pub max_file_length: u64,                    // → harper-ls maxFileLength
}

/// Lifecycle news the client thread pushes to the loop. Handled by `apply_provider_event`
/// (a thin reduce-arm delegation, module-structure GATE).
#[derive(Clone, Debug)]
pub enum ProviderEvent {
    /// Child crashed and was respawned within budget; open docs were re-opened lazily.
    Restarted,
    /// Terminal failure: not installed / respawn budget exhausted. Carries the status hint.
    Degraded(String),
}
```

(No `DictAddFailed`: add-to-dict's authoritative write is our own `append_word_to_dict`, whose
`io::Result` surfaces synchronously exactly as today; the server-side reload is best-effort and
its failure is immaterial — §7.4. This removes the round-1 double-write risk at the source.)

Placement: `Editor` gains one field (after `diag`, near the other diagnostics state):

```rust
/// The active diagnostics provider (Effort A). NullProvider by default so tests and
/// pre-config construction are hermetic; run() installs HarperLs after the msg channel exists.
pub diag_provider: Box<dyn crate::diag_provider::DiagnosticsProvider>,
```

`Editor` derives `Debug` (editor.rs: `#[derive(Debug)] pub struct Editor`), hence the
`std::fmt::Debug` supertrait. `Editor::new_from_text` initializes the field to
`Box::new(NullProvider)`.

`NullProvider` (in `diag_provider.rs`, production code — it is the default wiring, not a test
double): `name() = "none"`, `availability() = Availability::Idle`, every other method a no-op.
With `Availability::Idle` and no emissions, a test Editor never spawns anything and never shows
provider status — hermetic by default (the e2e harness additionally keeps
`diag_cfg.enabled = false` as today).

**Mock for tests:** a `#[cfg(test)]`-gated `RecordingProvider` in `diag_provider.rs` records
every call (`Vec<ProviderCall>`) and exposes a settable `availability` — Fs-seam/M3 precedent so
CI never needs harper-ls installed. Dispatch-path unit tests install it via
`editor.diag_provider = Box::new(...)`.

**`apply_provider_event(editor: &mut Editor, ev: ProviderEvent, clock: &dyn Clock)`** (also
`diag_provider.rs`). It takes a `clock` because `Restarted` re-arms, and `DiagStore::arm` needs a
timestamp — `reduce` already has `clock: &dyn Clock` in scope (verified: `reduce(msg, editor, reg,
keymap, ex, clock, msg_tx)`), so the reduce/prompts arms pass it through:

- `Restarted` → `editor.status = "grammar checker restarted".into()`; if
  `should_run_diagnostics(editor)`, re-arm the active buffer. **Copy `now`/`debounce` out before
  the mutable borrow** (mirroring the real `arm_if_edited`, which copies `debounce_ms` first —
  otherwise `active_mut()` and the `editor.diag_cfg` read alias):
  ```rust
  if crate::diagnostics_run::should_run_diagnostics(editor) {
      let now = clock.now_ms();
      let debounce = editor.diag_cfg.debounce_ms;
      editor.active_mut().diagnostics.arm(now, debounce);
  }
  ```
  so underlines self-heal without waiting for the next edit.
- `Degraded(hint)` → `editor.status = hint` (e.g. `"grammar checker unavailable — install
  harper-ls (Arch: pacman -S harper)"`).

The clearing of stuck `in_flight_version`s does NOT live here — the client thread guarantees a
(possibly empty) `Msg::DiagnosticsDone` for every **accepted** change (§3.4 + the §5 latch
invariant), which clears `in_flight_version` through the existing `apply_diagnostics_done` path.
One recovery mechanism, not two.

---

## 3. The harper-ls client (`harper_ls.rs`)

### 3.1 App-side handle

```rust
#[derive(Debug)]
pub struct HarperLs {
    cmd_tx: std::sync::mpsc::Sender<Inbound>,   // shared with the reader thread (Inbound::Cmd here)
    shared: std::sync::Arc<Shared>,             // availability mirror
    started: bool,                              // ensure_running latch (app-thread-local)
    msg_tx: std::sync::mpsc::Sender<crate::app::Msg>,
    cfg: crate::diag_provider::ProviderConfig,
}

#[derive(Debug)]
struct Shared { availability: std::sync::Mutex<crate::diag_provider::Availability> }
```

Constructed in `app.rs::run()` right after the msg channel exists (the
`let (msg_tx, msg_rx) = std::sync::mpsc::channel::<Msg>();` site) and installed with its config:

```rust
editor.diag_provider = Box::new(crate::harper_ls::HarperLs::new(
    msg_tx.clone(),
    crate::diag_provider::ProviderConfig {
        grammar: cfg.diagnostics.grammar,
        dictionary: cfg.diagnostics.dictionary.clone(),
        max_file_length: crate::limits::HARPER_MAX_FILE_LENGTH,
    },
));
```

`HarperLs::new` is cheap: it creates the channel pair and the `Shared`; **no thread, no process**.
`ensure_running` spawns the `wcartel-harper-client` thread on first call and sets the `started`
latch **only when `std::thread::Builder::spawn` returns `Ok`** — a spawn `Err` instead sets
`availability = Unavailable` and leaves `started` false, so no `Cmd::Change` is ever accepted
against a thread that never came up (round-3 spawn-failure coverage).
`notify_change` forwards `Inbound::Cmd(Cmd::Change{..})` over `cmd_tx` and returns
`Accepted::Yes` iff the `send` succeeded (`Ok`). A disconnected send (`Err` — the client thread
has ended) also **flips the shared `availability` to `Unavailable`** (belt-and-suspenders — the
thread's own exit path already did so) and returns `Accepted::No`, so the caller never sets the
in-flight latch on a dead thread (the §5 latch invariant). The other, fire-and-forget trait
methods (`configure`/`notify_close`/`reload_dictionary`/`shutdown`) ignore a disconnected send —
they have no latch to protect. `availability()` reads the mutex (uncontended; a lock per render
frame is cold-path cheap and only taken while Review is the active mode — see §10).

Commands sent **before** the handshake completes queue naturally in the state machine (§3.3) and
replay after `initialized` — this is what makes lazy spawn + "no silent wait" work: the first
`notify_change` is accepted immediately and served when the server is up. (While the thread is
alive but `Starting`, the `send` succeeds → `Accepted::Yes`; the queued change is guaranteed a
terminal emission once the handshake completes and it replays, so the latch is safe to set.)

### 3.2 Thread family and child process

On `ensure_running` (first call), one thread spawns: **`wcartel-harper-client`**. It:

1. Spawns the child: `std::process::Command::new("harper-ls").arg("--stdio")` with
   `stdin/stdout = piped`, `stderr = null`. Spawn error of kind `NotFound` → this IS the
   runtime PATH detection (no TOCTOU pre-probe): set `Unavailable`, emit
   `ProviderEvent::Degraded(install hint)`, drain-and-drop remaining commands, exit thread.
   Other spawn errors count against the respawn budget (§3.4).
2. Spawns **`wcartel-harper-read`**: a reader owning the child's stdout, looping
   `lsp_rpc::read_frame` and forwarding `Inbound::Server(value)`; on read error/EOF it sends
   `Inbound::ServerEof` and exits. (Mirror of the M4 input-thread + watchdog shape in
   `app.rs::run`: a blocked reader whose death is surfaced as a message, never a hang.)
3. Runs the pump: `recv_timeout(until next state-machine deadline)` over the single `Inbound`
   channel (app commands + server traffic + EOF), feeding `HarperState` and executing the
   actions it returns (write frame to child stdin, emit `Msg`, set availability, respawn, exit).
   Timeouts feed `HarperState::on_deadline(now)` (publish/codeAction watchdogs, §3.4). With
   nothing pending, the pump blocks indefinitely on `recv` — **idle is free**.

**Thread-exit flush — the last leg of the latch invariant (round-3 hardened).** The pump body runs
inside a **`FlushGuard`** RAII struct that **owns `cmd_rx`** and holds the `msg_tx` and a
`&mut HarperState`; the pump loop receives via `guard.cmd_rx`. On *every* exit — clean return,
degrade, **or panic-unwind** (`std::panic::catch_unwind` wraps the pump so a panic cannot bypass
the guard) — `Drop` performs a **two-part flush** that emits an empty version-tagged
`Msg::DiagnosticsDone` for every version that could possibly be owed, no matter where the thread
died relative to reading the channel:

1. **Tracked flush:** every `awaiting_publish`, `assembling`, and queued (`queued`) entry in
   `HarperState` — the versions the pump has already recorded.
2. **Channel drain (closes the round-3 "accepted-but-unrecorded" gap):** because `cmd_tx.send(Ok)`
   only proves a `Cmd::Change` *entered the channel*, not that the pump moved it into `awaiting`,
   the guard **drains `cmd_rx` with `try_recv()`** and emits an empty terminal for every remaining
   `Cmd::Change { buffer_id, version, .. }` still in the queue. Since the guard owns `cmd_rx`, this
   drain runs before the receiver is dropped, and any later app-side `send` (post-death) returns
   `Err` → `Accepted::No` → no latch (§3.1).

To close the last sub-window — a panic *between* `recv()` returning a `Cmd::Change` and
`HarperState` recording it — the pump makes **recording the awaiting slot the first, non-IO step**
of handling a change (`on_change` writes `awaiting_publish[buffer]` before any `Send` action is
executed), so a received-but-unhandled change is either still in the channel (drained by part 2) or
already tracked (flushed by part 1). Thus: **once dispatch latches `in_flight_version = Some(v)`, a
terminal `DiagnosticsDone` for `v` is emitted regardless of where the worker died** — before,
during, or after reading the channel, including a child-spawn failure (the thread still runs its
`Drop`) . Thread-*spawn* failure is handled one level up: `ensure_running` sets `Unavailable` and
does not latch `started` if `thread::Builder::spawn` returns `Err`, so no change is ever accepted
against a thread that never existed (§3.1). (`catch_unwind` requires the captured state
`AssertUnwindSafe`; `HarperState` + `Sender`s + `Receiver` are owned data, so this is sound — the
M4 worker-panic-isolation precedent.)

```rust
enum Inbound {
    Cmd(Cmd),                      // from the app-side handle
    Server(serde_json::Value),     // one parsed JSON-RPC frame
    ServerEof,                     // reader ended (child died or closed stdout)
}
enum Cmd {
    Configure(ProviderConfig),
    Change { buffer_id: BufferId, version: u64, path: Option<PathBuf>, text: String },
    Close { buffer_id: BufferId },
    ReloadDict,                    // best-effort didChangeConfiguration resend (not a writer)
    Shutdown,
}
```

### 3.3 The `HarperState` protocol state machine (pure, unit-testable)

All protocol logic lives in a struct with **no IO**: inputs are `Inbound` values + `now_ms`,
outputs are `Vec<Action>`. The thread is a thin pump. This is what makes the client testable
without a process (§15) and keeps each function small.

```rust
enum Action {
    Send(serde_json::Value),           // one frame to child stdin
    Emit(crate::app::Msg),             // DiagnosticsDone / DiagProviderEvent
    SetAvailability(Availability),
    Respawn,                           // pump: kill+reap child, respawn, re-handshake
    Exit,                              // pump: kill child if any, end thread
}

struct HarperState {
    phase: Phase,                      // Initializing | Running | ShuttingDown
    cfg: ProviderConfig,
    docs: HashMap<BufferId, DocState>, // uri: String, lsp_version: i32, our_version: u64, generation: u64, text: String, open: bool
    uri_owner: HashMap<String, (BufferId, u64)>, // wire uri → (buffer, generation): the stale-publish discriminator (§5)
    next_generation: u64,              // monotonic; every didOpen/reopen takes the next value, never reused
    queued: Vec<Cmd>,                  // commands received before Running
    next_id: u64,                      // JSON-RPC request ids we allocate
    pending_requests: HashMap<u64, PendingKind>, // Initialize | Shutdown | CodeAction{buffer_id,generation}
    awaiting_publish: HashMap<BufferId, AwaitPublish>, // our_version, generation, deadline_ms
    assembling: HashMap<BufferId, Assembly>, // converted diags + our_version + generation + deadline, keyed while codeAction is out
}
```

**Document generation — the wire-level stale discriminator.** Every doc carries a
`generation: u64` drawn from the monotonic `next_generation` counter on each `didOpen`/reopen, and
the generation is **embedded in the wire URI** so the server tags every publish with it implicitly
(the uri is the tag). This is the load-bearing invariant behind §5: a publish for a superseded
generation carries a uri no longer in `uri_owner` → dropped, with zero dependence on whether
harper-ls echoes `PublishDiagnosticsParams.version`.

**URIs are opaque, generation-tagged, and identical in form for saved and unsaved documents:**

```
lsp_rpc::doc_uri(buffer_id, generation) -> String  =  format!("untitled:wcartel-{}-{}", buffer_id.0, generation)
```

The URI is **not** derived from the file path. Two reasons, both now **empirically verified against
harper-ls 2.1.0** (local stdio probe): (a) harper-ls lints an `untitled:` document sent with
`languageId="markdown"` identically to a `file://` one — it lints the `didOpen`/`didChange` text +
languageId, not the file at the path; and (b) `userDictPath` applies to `untitled:` documents (a
custom-dictionary word was not flagged in an untitled doc). So the opaque form is a checked fact,
not an assumption. This choice **dissolves two round-2 findings by construction**: there is no
`url::Url::from_file_path` call, so a real *relative* `Document.path` (wordcartel stores the CLI
arg verbatim — `wcartel notes.md` → `path = "notes.md"`, never canonicalized) cannot fail URI
construction (round-2 #2); and because the form is always accepted, there is no probe-gated URI
variant that a first dispatch could get wrong (round-2 #4). The `url` crate is therefore **not** a
dependency.

Because the client hand-rolls the JSON-RPC envelope over `serde_json::Value`, URIs live on the
wire as plain JSON strings — no `lsp_types::Uri`/`Url` construction anywhere. `DocState.uri` is a
`String`; an incoming publish uri is matched by exact string equality against `uri_owner`. The
probe confirmed harper **echoes the URI verbatim** (including our opaque generation tag) in
`publishDiagnostics`, so URI-keyed acceptance holds exactly. Save/save-as is invisible to the URI
(same buffer_id, same generation) → a save is just a normal edit-driven `didChange`, no reopen and
no migration.
```

**Handshake.** On (re)spawn the pump feeds `HarperState::on_spawned(now)`:
`initialize` request (params: `process_id = Some(std::process::id())`, `root_uri = None`,
`client_info = wordcartel + version`, `initialization_options = None`, capabilities:
`text_document.publish_diagnostics.version_support = true`, `text_document.code_action` present,
`workspace.did_change_configuration` present, and — **critically —
`workspace.configuration = true`** so the server will PULL config from us, §8). On the
`initialize` response → `initialized` notification → `workspace/didChangeConfiguration` with the
settings object (§8) → phase `Running` → replay `queued` in order. **The push alone delivers
nothing** — harper-ls only actually reads config by *pulling* it back via
`workspace/configuration` requests (see below); the push is the trigger that makes it (re-)pull.

**Text sync (FULL — grounded constraint; harper-ls does not do incremental).**
`Cmd::Change { buffer_id, version, path, text }` (the `path` field is carried for future use but
does not affect the URI — §above):

- Needs a (re)open only when the buffer has no live `DocState` (first check, or after a
  `notify_close` from reload/recover, §4.13). A reopen assigns a fresh generation. There is no
  path-driven reopen: a save/save-as keeps the same buffer_id + generation, so it is an ordinary
  `didChange`.
- (Re)open → take `generation = next_generation` (post-increment), compute
  `uri = doc_uri(buffer_id, generation)`, register `uri_owner[uri] = (buffer_id, generation)`,
  send `textDocument/didOpen` (`language_id: "markdown"`, `version: lsp_version`, full text).
  Already open → `textDocument/didChange` with one full-range
  `TextDocumentContentChangeEvent { range: None, text }` at the doc's current uri/generation.
- `lsp_version` is a **client-local i32 counter per doc** (`+1` per send, starting at 1) — NOT
  `version as i32` (u64→i32 truncation is exactly the H7 arithmetic class we audit against).
  Increment via `saturating_add(1)` and, per the H7 stance, a `debug_assert!(lsp_version <
  i32::MAX)` — a per-edit counter cannot realistically reach 2³¹ in a session, but the saturate
  makes the release behavior defined (it pins at `i32::MAX`; a pinned value still round-trips as a
  valid LSP version and, since `version` echo is unused anyway (§Receive step 2), correctness of
  acceptance never depends on it — generation is the tag). `DocState` records
  `(uri, lsp_version, our_version, generation, text, open)`; `text` is the sent
  string, kept for position conversion (§6).
- Record `awaiting_publish[buffer_id] = { our_version: version, generation, deadline: now + PUBLISH_TIMEOUT_MS }`.

`Cmd::Close` → **first emit the terminal, then remove state** (round-3 #2; resolves the §5-vs-§3.3
contradiction): for any `awaiting_publish`/`assembling` entry for this `buffer_id`, emit an empty
`Msg::DiagnosticsDone { buffer_id, version: our_version, .. }` so a latched in-flight version is
guaranteed its terminal; *then* send `didClose`, remove `DocState`, its `uri_owner` entry, and the
awaiting/assembly entries. This matters because `notify_close` is used by reload/recover (§4.13),
which **reuses the same `BufferId`** with a bumped `document.version`: the emitted empty terminal is
tagged with the *old* version and is dropped by `apply_diagnostics_done`'s version gate (the buffer
is now at v+1), but it still clears the old `in_flight_version` if it were somehow current — and,
for a genuine buffer-close (distinct id retired), the `by_id_mut` simply finds nobody. Either way,
a `Cmd::Close` never silently strands an accepted in-flight change (the §5.1 latch invariant).

**Receive.** `publishDiagnostics` for uri U:

1. **Generation attribution (the airtight tag).** Look up `uri_owner[U]`. Absent → the publish is
   for a closed or superseded-generation document → **drop it outright** (this single check kills
   the reload/recover stale-publish race of §5, independent of version echo). Present as
   `(B, g)` with `g == docs[B].generation` and `docs[B].open` → this publish belongs to the live
   document; tag = `docs[B].our_version`. (A `uri_owner` hit whose `g` is stale can only occur
   transiently and is likewise dropped — we remove the entry at close/reopen.)
2. **Version cross-check (secondary guard — and empirically a no-op on harper 2.1.0).** The probe
   showed harper-ls 2.1.0 **omits `PublishDiagnosticsParams.version` (always `None`)**, which is
   exactly why generation-in-URI (step 1) is the load-bearing discriminator and not optional. When
   the field IS present on some future/other version and `v != docs[B].lsp_version`, we drop (a
   pre-latest snapshot; the watchdog bounds the wait); when omitted (the harper-2.1.0 case) we
   accept the generation tag from step 1. This guard is *additional* correctness only — §5's proof
   never relies on it.
3. Convert each LSP diagnostic against `docs[B].text` (the exact text of the tagged version):
   UTF-16 range → byte range (§6; unconvertible → drop that diagnostic), classify
   Spelling/Grammar (§7.1), apply the grammar gate (§7.2: `!cfg.grammar` drops Grammar).
4. Empty result → emit `Msg::DiagnosticsDone { buffer_id, version: tagged, diagnostics: vec![] }`
   immediately and clear `awaiting_publish[B]`.
5. Non-empty → **eager-assemble**: one batched `textDocument/codeAction` request —
   `range` = envelope over all converted diagnostics (min start .. max end, converted back to the
   same UTF-16 positions the server sent — we reuse the server's own positions to avoid a
   round-trip conversion error), `context.diagnostics` = the publish's raw LSP diagnostics.
   Park the converted set in `assembling[B]` with `{ our_version: tagged, generation: g,
   deadline: now + CODEACTION_TIMEOUT_MS }`; clear `awaiting_publish[B]` (the publish arrived; the
   assembly watchdog takes over). The parked `generation` is re-checked on the response: if the
   doc's generation advanced meanwhile (a close+reopen raced the codeAction), the assembly is
   discarded — never emitted against a newer generation.
6. On the codeAction **response** (structure **empirically verified**, harper-ls 2.1.0): each
   fix arrives as `CodeAction { kind: "quickfix", edit: { changes: { <uri>: [ { newText, range } ]
   } } }` with **clean structured `newText`** (e.g. `"the"`, `"tea"`, `"tech"`), NOT a display
   string. Handling: keep only actions with `kind == "quickfix"` **and** a non-empty `edit`; for
   each, take `edit.changes[our_uri][].newText` + its UTF-16 `range`, convert the range to bytes
   (§6), and map to `Suggestion::ReplaceWith(newText)` (§6.2). **Drop the command-only actions**
   ("Ignore Harper error", "Add … to user/workspace/file dictionary" — probe shows these have
   `kind: None`, an empty `edit`, and a `command` set): we supply ignore/add-dict client-side.
   Never read the action `title` (its typographic quotes are cosmetic — the `edit.newText` is the
   clean value). Attach each suggestion to its diagnostic — matching precedence: (a) the action's
   `diagnostics` field by (range, message) equality against the publish set; (b) else the unique
   diagnostic whose range overlaps the edit's range; no unique match → drop the suggestion (the
   diagnostic still paints). Then emit `Msg::DiagnosticsDone { buffer_id, version: tagged,
   diagnostics }` with suggestions attached, sorted ascending by `range.start` (preserving the
   documented `check` ordering contract that `diag_next`/`diag_prev` and render assume).
7. codeAction **error response or timeout** → emit the converted set with empty `suggestions`
   (underlines still paint; the overlay simply offers only ignore/add rows). Never stall the
   store on a fix-fetch failure.

**Server-initiated requests — the config PULL responder is FIRST-CLASS (round-3 B, MUST-FIX,
empirically verified).** harper-ls uses the LSP configuration **pull** model: on init and per-doc
it sends `workspace/configuration` **requests** (server→client, carrying an `id`) and *waits* for
the client's response. **A `didChangeConfiguration` push alone delivers nothing** — in the probe,
until the client answered the pull, harper-ls produced **zero diagnostics and ignored
`userDictPath` entirely** (every custom word flagged). So the responder is load-bearing, not an
afterthought:

- `HarperState` answers every inbound `workspace/configuration` request with a **result array — one
  entry per `params.items` entry — where each entry is the BARE, UNWRAPPED harper settings object**
  `{ dialect, userDictPath, linters… }`, **NOT** wrapped in `{"harper-ls": {…}}` (empirically
  verified, harper-ls 2.1.0: the request sends `params.items = [{}]` with an empty/absent `section`,
  and only the unwrapped response made `userDictPath` apply and diagnostics flow). This is a
  `Send(response)` action keyed to the echoed request `id`. Note the asymmetry: the
  `didChangeConfiguration` PUSH object IS nested under `"harper-ls"` (§8) but does not by itself
  deliver config — the unwrapped PULL RESPONSE is the load-bearing one.
- Correctness thus depends on this handler as much as on `didOpen` — a missing/oversimplified
  responder is a silent functional break (defaults to American/all-linters/no-dictionary). The
  handler is exercised by a dedicated `HarperState` test (§15) and the dev-machine integration
  test.
- Other server requests: `window/workDoneProgress/create` and `client/registerCapability` → null
  result; any other request → JSON-RPC `MethodNotFound` error. Unknown notifications are ignored.
  (The reader thread forwards every server frame — request, response, or notification — as
  `Inbound::Server`; the pump routes requests here, responses to `pending_requests`.)

**ReloadDict (best-effort, non-writer).** `Cmd::ReloadDict` → resend
`workspace/didChangeConfiguration`; harper reacts by **re-pulling** via `workspace/configuration`,
which our responder answers with the (unchanged) settings incl. `userDictPath`, causing harper to
re-read the dictionary file *our* `append_word_to_dict` just wrote (§7.4). Fire-and-forget: no
reply is awaited, and a failure is immaterial because the client-side apply filter already
suppresses every dictionary word in the UI (§7.3). We deliberately do **not** call
`executeCommand HarperAddToUserDict` — that command *appends* to `userDictPath`, which combined
with our own write would double-write the same file (round-2 #3). Single writer, no duplication.
(Whether harper re-reads `userDictPath` on this resend is the one remaining dict question for the
plan's packaged-version reconfirm, §16; if it doesn't, server-side suppression simply lags to the
next `didOpen`/restart — never visible, since the client filter hides the word meanwhile.)

**Shutdown.** `Cmd::Shutdown` → phase `ShuttingDown`, send `shutdown` request; on its response
(or after `SHUTDOWN_GRACE_MS`) send `exit` notification and `Exit`. The pump kills the child if
it hasn't exited after a bounded `try_wait` loop (≤ `SHUTDOWN_GRACE_MS`), reaps it, and ends the
thread. The app side never joins: `run()` calls `editor.diag_provider.shutdown()` on loop exit
(both the normal-quit and `InputLost` paths, before `TerminalGuard` teardown) and proceeds; if
the process exits first, the child sees stdin EOF and terminates itself (harper-ls exits on
closed stdin). Quit latency is therefore zero-added.

### 3.4 Crash → bounded respawn → degrade; the no-stall guarantees

`Inbound::ServerEof`, a stdin write error, or a framing-corrupt stream (unparseable frame) all
mean the server is gone or unusable:

- **Respawn budget:** `MAX_SPAWN_ATTEMPTS = 3` per session (the initial spawn counts as the
  first). Attempts remaining → `SetAvailability(Starting)`, `Respawn`; the pump kills/reaps and
  re-spawns, the state machine resets to `Initializing`, marks every `DocState.open = false`
  (they re-`didOpen` lazily on their next `Cmd::Change`), and re-queues its config push. Emit
  `ProviderEvent::Restarted`. Budget exhausted (or spawn itself failed post-first-attempt) →
  `SetAvailability(Unavailable)`, emit `ProviderEvent::Degraded(hint)`, drain remaining commands
  as no-ops, `Exit`. This mirrors the M4 input-thread supervision stance: death is surfaced as a
  message and handled in the loop, never a silent hang; the budget prevents a crash-loop.
- **In-flight recovery:** on either path — and on the thread-exit drop-guard flush (§3.2), which
  covers even a *panic* — every entry in `awaiting_publish`, `assembling`, and any unprocessed
  queued `Cmd::Change` emits an **empty** `Msg::DiagnosticsDone` tagged with its `our_version`.
  That flows through `apply_diagnostics_done`, which clears `in_flight_version` (and stores the
  empty vec — invalid for painting by `valid_for`'s non-empty rule, exactly like a clean
  no-findings result). The single-in-flight pipeline can therefore never wedge on a crash. This is
  the thread-side half of the §5 latch invariant (the dispatch-side half — never latching without
  acceptance — is §4.3). `Restarted`'s re-arm (§2) then schedules a fresh check.
- **Publish watchdog:** if the server never publishes for a sent version (e.g. a document over
  its `maxFileLength` is silently skipped — observed harper-ls behavior), the
  `awaiting_publish` deadline (`PUBLISH_TIMEOUT_MS = 10_000`) fires and emits the empty
  `DiagnosticsDone` for the tagged version — same unwedging path, no respawn. A late genuine
  publish after the timeout is still correctly tagged and, if the buffer hasn't changed, applies
  (underlines appear late rather than never).
- **codeAction watchdog:** `CODEACTION_TIMEOUT_MS = 5_000` → emit without suggestions (§3.3.7).

Constants live in `harper_ls.rs` as module consts with one-line rationales;
`HARPER_MAX_FILE_LENGTH` and the client-side send cap live in `limits.rs` (§8.3).

---

## 4. Integration: every touched production site

The orchestration layer keeps its shape; this section is the exhaustive touch list.

1. **`wordcartel-core/src/diagnostics.rs`** — delete `check`, `CheckOpts`, `HarperLint`,
   `harper_lints`, `classify`, `char_span_to_bytes`, `map_suggestions`, and the `harper_core`
   imports + the Harper-driving tests. Keep `Diagnostic`, `DiagnosticKind`, `Suggestion` with
   their derives, doc comments updated (the module doc no longer says "Wraps harper-core"; it
   documents the pure data contract: byte ranges into the checked text, sorted by `range.start`).
2. **`wordcartel-core/Cargo.toml`** — remove `harper-core = "2"`. (`Cargo.lock` drops the burn
   tree; the H2 research item closes via this effort's ledger note.)
3. **`wordcartel/src/diagnostics_run.rs`** —
   - `DiagStore`, `valid_for`, `arm`, `next_deadline`, `should_run_diagnostics`,
     `should_show_diagnostics`, `arm_if_edited`, `diag_due` — **unchanged**.
   - `dispatch_diagnostics` re-pointed at the seam (signature shrinks — no cfg/ignore/msg_tx):

     ```rust
     /// Consume the armed deadline and hand the buffer to the diagnostics provider.
     /// Sets in_flight_version only when a notify was actually enqueued; the provider
     /// guarantees a (possibly empty) DiagnosticsDone for every accepted version.
     pub fn dispatch_diagnostics(editor: &mut Editor) {
         let b = editor.active();
         let (buffer_id, version) = (b.id, b.document.version);
         let path = b.document.path.clone();
         let text = b.document.buffer.snapshot().to_string();
         editor.active_mut().diagnostics.recheck_due_at = None; // consumed
         if text.len() as u64 > crate::limits::DIAG_MAX_SEND_BYTES {
             editor.status = "document too large for grammar checking".into();
             return; // no in_flight; nothing outstanding
         }
         editor.diag_provider.ensure_running();
         if editor.diag_provider.availability() == crate::diag_provider::Availability::Unavailable {
             if !editor.diag_hint_shown {
                 editor.diag_hint_shown = true;
                 editor.status = crate::diag_provider::INSTALL_HINT.into();
             }
             return; // no in_flight
         }
         if editor.diag_provider.availability() == crate::diag_provider::Availability::Starting {
             editor.status = "starting grammar checker…".into(); // no silent wait
         }
         // LATCH INVARIANT (§5): set in_flight_version ONLY on Accepted::Yes. On Accepted::No the
         // thread died between the availability read and the send — no terminal DiagnosticsDone
         // would ever arrive, so latching here would wedge diagnostics permanently. Instead:
         // leave the latch clear (a fresh dispatch retries) and surface the degrade hint.
         match editor.diag_provider.notify_change(buffer_id, version, path, text) {
             crate::diag_provider::Accepted::Yes => {
                 editor.active_mut().diagnostics.in_flight_version = Some(version);
             }
             crate::diag_provider::Accepted::No => {
                 // availability() is now Unavailable (set by notify_change on the disconnected send)
                 if !editor.diag_hint_shown {
                     editor.diag_hint_shown = true;
                     editor.status = crate::diag_provider::INSTALL_HINT.into();
                 }
             }
         }
     }
     ```

     Notes: on the very first dispatch `ensure_running` flips `Idle → Starting`, so the
     "starting grammar checker…" status paints on the same tick the user entered Review
     (`set_render_mode` arms at debounce 0 → `timers::next_wake` wakes immediately). The
     `Starting` branch's notify queues in the client and is served post-handshake — first-Review
     init cost lives in the subprocess, invisible to typing. The `Accepted::No` arm is the wedge
     guard: it fires only in the narrow window where the client thread died after the availability
     read; the very next armed dispatch either respawns (if within budget) or shows the hint.
   - `apply_diagnostics_done` — same version gate + in-flight clear, plus the **ignore filter at
     apply time** (§7.3): after the version check passes, retain only diagnostics not ignored
     (`kind != Spelling ||` surface word ∉ `editor.dictionary ∪ editor.session_ignores`,
     case-insensitive, surface sliced from the live buffer with the existing clamp discipline).
   - `append_word_to_dict` — **retained** as the proven, no-data-loss dictionary writer (§7.4);
     it is called from add-to-dict alongside the provider's own reload trigger.
   - New small helper `retain_unignored(editor)` — in-place refilter of the active `DiagStore`
     used by the ignore/add-dict overlay rows (§7.3).
4. **`wordcartel/src/timers.rs`** — `on_tick`'s diagnostics block simplifies to
   `if should_run_diagnostics(editor) && diag_due(...) { dispatch_diagnostics(editor); }`
   (the `ignore_words` Arc build over `editor.dictionary ∪ session_ignores` and the
   `diag_cfg.clone()` disappear). `diag_deadline` and the `SUBSYSTEMS` row are unchanged — the
   in-flight/no-spin invariants carry over verbatim because `in_flight_version`'s
   set/clear discipline is preserved.
5. **`wordcartel/src/lib.rs`** — add the three module declarations beside the existing explicit
   `pub mod diagnostics_run; … pub mod diag_overlay;` list: `pub mod diag_provider;`,
   `pub mod lsp_rpc;`, `pub mod harper_ls;`. (The crate declares every module explicitly here —
   without this the new modules don't compile in.)
6. **`wordcartel/src/app.rs`** —
   - Delete the `wcartel-diag-warm` startup thread block (its whole reason — pre-warming the
     in-process `FstDictionary` — left the building; lazy spawn replaces it).
   - **Keep** the startup personal-dictionary load into `editor.dictionary` (the
     `bounded_read_opt(dict_path, …)` block) — it is now the no-data-loss client-side suppression
     seed (§7.4), unioned into the ignore filter so existing users' saved words are never
     re-flagged regardless of harper-ls's own dictionary consumption.
   - Install `HarperLs` right after `msg_tx` is created (§3.1).
   - `Msg` gains `DiagProviderEvent(crate::diag_provider::ProviderEvent)` with a manual `Debug`
     arm (matching the existing hand-written `impl Debug for Msg`). The **message match lives in
     `reduce_dispatch`** (not `reduce` — `reduce_dispatch(msg, editor, reg, keymap, ex, clock,
     msg_tx)` holds the `Msg` match; `clock` is a parameter there, verified). Add the thin arm
     `Msg::DiagProviderEvent(ev) => crate::diag_provider::apply_provider_event(editor, ev, clock),`
     to that match, beside `Msg::DiagnosticsDone`.
   - Both run-loop exit paths call `editor.diag_provider.shutdown()`.
   - Net app.rs line delta ≈ 0; the 1000-line hub budget is untouched.
7. **`wordcartel/src/prompts.rs::intercept`** — add the SAME arm to the modal-prompt interceptor's
   match (round-3 #3 — the arm must be in **both** `reduce_dispatch` AND `prompts::intercept`):
   `Msg::DiagProviderEvent(ev) => crate::diag_provider::apply_provider_event(editor, ev, clock),`
   beside its existing `JobDone`/`FilterDone`/`ExportDone`/`TransformDone`/`DiagnosticsDone`/
   `ClipboardPaste` arms (`intercept(msg, editor, ex, clock, msg_tx)` threads `clock`, verified),
   so `Degraded`/`Restarted` reach the status line even while a modal prompt is open (e.g. harper
   crashes during a quit/save prompt).
   **Intercept-site sweep (precise — round-2 #5 / round-3 #4 wording).** Several interceptors
   consume *various* background/paste/mouse variants — `menu`/`palette`/`theme_picker`/
   `file_browser` early-return on `Msg::ClipboardPaste`, `splash` swallows paste/mouse — but **none
   of them matches `Msg::DiagProviderEvent`**, so the new variant falls to each one's tail
   `Handled::Pass(msg)` and reaches `reduce_dispatch`'s arm. The single exception is `prompts.rs`,
   whose catch-all `_ => {}` would swallow it — hence the added arm there. `diag_overlay`/
   `search_ui`/`minibuffer`/`outline_overlay` intercept KEY input only and `Pass` every non-key
   message; `mouse.rs` has no message-level interceptor. **Invariant: `Msg::DiagProviderEvent`
   reaches `reduce_dispatch` through every interceptor except `prompts.rs` (now handled).**
8. **`wordcartel/src/editor.rs`** — the `diag_provider` field (§2); **keep**
   `pub dictionary: HashSet<String>` (§7.4 no-loss seed; `session_ignores` also stays); add
   `pub diag_hint_shown: bool` (default `false`), reset to `false` inside `set_render_mode` when
   the mode being set is `Review` (one hint per deliberate Review entry). `set_render_mode`'s
   arm-on-enter behavior is otherwise unchanged.
9. **`wordcartel/src/search_ui.rs::diag_apply_selected`** — the three branches keep their guard
   structure (opened_version staleness + clamp); bodies change:
   - **ignore once** — unchanged semantics, better mechanics: insert into `session_ignores`,
     close the overlay, then `retain_unignored(editor)` (immediate in-place refilter — no server
     round-trip; the old re-arm is dropped because a full re-check to remove one underline is
     pure waste under LSP full-doc sync). Ephemeral-only, exactly today (locked F4: harper's own
     ignore persists — semantic mismatch — so it is never used).
   - **add to dictionary** — **single writer, no double-write** (round-2 #3): the *only* write is
     our own `append_word_to_dict(dict_path, &word)` (authoritative persist to the file that is
     also harper-ls's `userDictPath`); `editor.dictionary.insert(word)` gives instant client-side
     suppression; then `editor.diag_provider.reload_dictionary()` best-effort nudges harper to
     re-read that same file (a `didChangeConfiguration` resend — **not** a second write; harper's
     own appending `HarperAddToUserDict` command is deliberately never called). Close,
     `retain_unignored(editor)`. The existing "no dictionary path configured" branch stays for the
     `dictionary = None` case (harper falls back to its own default path; §8.1) — in that case we
     skip both the write and the reload nudge but still insert into `editor.dictionary` for
     session suppression. A local write failure surfaces synchronously on the status line exactly
     as today; harper's server-side view is immaterial (the client filter hides the word), so
     there is no async failure path to surface.
   - **suggestion apply** — byte-for-byte unchanged (`build_range_replace` → Transaction →
     `apply`): eager-assembly's whole purpose.
10. **`wordcartel/src/workspace.rs::close_buffer_now`** — call
    `editor.diag_provider.notify_close(id)` in **all three** close shapes so the server never
    keeps a closed doc open until shutdown: the two removal branches (active-removal via
    `switch_to`, and inactive-removal) AND the **replace-last-ordinary-buffer branch**, where the
    slot is *replaced* with a fresh `BufferId` rather than removed — `notify_close(id)` for the
    old id must fire before `editor.buffers[i]` is overwritten (the new empty buffer will re-open
    lazily under its own new id/generation on its next Review dispatch). Verify the exact
    insertion points against the fn body at plan time.
11. **`wordcartel/src/render_status.rs::status_left_text`** — the Review arm of `mode_text`
    becomes attribution-aware (§10).
12. **`wordcartel/src/registry.rs`** — no changes: `quick_fix`/`diag_next`/`diag_prev` guards
    (`should_show_diagnostics` + `valid_for`) and `recheck_diagnostics` (arm at 0) are
    provider-agnostic already. A degraded provider simply never repopulates the store; a manual
    recheck then shows the §4.3 unavailable hint via dispatch. (Deliberate: no auto-resurrect on
    recheck — a crash-looping server stays down for the session.)
13. **`wordcartel/src/save.rs`** — `reload_from_disk` and `load_recovered` (the two sanctioned
    wholesale-replacement paths) each add **`editor.diag_provider.notify_close(id)`** as part of
    the replacement (right where they reset `DiagStore::new()` and bump `document.version`), so
    the provider abandons the pre-reload generation and reopens fresh (§5 item 4 — this is the
    round-1 Critical fix). The existing version-bump + `DiagStore` reset stay; they are the
    version-axis half of the double guard.
14. **`wordcartel/src/config.rs`** — no schema change. `DiagnosticsConfig` keeps
    `enabled, grammar, debounce_ms, dictionary, linters`; `linters` stays parsed-and-dormant
    until provider #2 (locked scope nit). `dictionary`'s doc comment is updated: it is now both
    the client-side suppression seed AND harper-ls's `userDictPath` (same file, same
    one-word-per-line format).
15. **`packaging/arch/PKGBUILD`** — `optdepends+=('harper: grammar/spelling diagnostics in
    Review mode (harper-ls language server)')`.

---

## 5. Async correctness: eager-assemble × staleness — the two-axis (generation + version) guard

Claim: **a `Msg::DiagnosticsDone` produced by this pipeline is either dropped or correct** —
byte ranges (and attached suggestions) always describe the exact buffer content of the version
they are tagged with. The failure mode of every race below is a wasted fetch or a delayed/absent
underline, never a wrong-range application. The proof rests on **two independent tags**, and the
result is accepted only when BOTH agree with the live buffer: the client-side **generation** (a
wire-embedded document epoch, §3.3) and `document.version` (the existing gate).

1. **The tag is bound to the text at conversion time.** The client converts LSP positions
   against `DocState.text` — the verbatim string sent for that doc's current generation — never
   against the live buffer. So *if attribution is right*, ranges are right by construction,
   including every multibyte case (§6). Suggestions are assembled onto those same converted
   diagnostics; the emitted set carries the doc's `our_version`. A parked assembly whose
   generation was superseded mid-fetch is discarded (§3.3.5), never re-tagged.
2. **Generation makes attribution airtight — WITHOUT relying on the server.** Every publish is
   attributed by its **wire URI**, which embeds the generation (§3.3). `uri_owner` maps a live
   uri → `(buffer, generation)`; a publish whose uri is absent (closed or superseded generation)
   is dropped in step 1 of Receive. This is the load-bearing guarantee and it is a pure
   client-side lookup — it holds whether or not harper-ls echoes `PublishDiagnosticsParams.version`
   (an R1 unknown). The version echo, when present, is only a *secondary* in-generation cross-check.
3. **The single-in-flight discipline bounds in-generation staleness.** Within one generation,
   `diag_due` refuses to dispatch while `in_flight_version.is_some()` (any version — the existing
   invariant, tested by `diag_due_requires_armed_reached_and_not_in_flight`), so at most one
   un-acknowledged `didChange` exists and the server's knowledge of the document *is* the last-sent
   text. Any publish it emits (didChange-, config-reload-, or dict-reload-triggered) is computed
   from that text, so tagging an omitted-version publish with the doc's current `our_version` is
   sound.
4. **The reload/recover race — the round-1 hole — is now closed on both axes.**
   `save.rs::reload_from_disk` and `load_recovered` preserve `BufferId`, bump `document.version`
   (+1), and reset `DiagStore::new()` (clearing `in_flight_version`) — which *by itself* would let
   an old-content publish be mis-accepted, because the reset removes the single-in-flight backstop
   and (since the server omits `version`, §Receive step 2) the awaiting slot would be reused for the
   new version. §4 closes it: both functions call **`editor.diag_provider.notify_close(id)`** as
   part of the wholesale replacement. `Cmd::Close` **emits the terminal for the outstanding version
   before removing state** (§3.3 — the round-3 #2 fix; this is why §3.3 and §5 no longer
   contradict), then (a) drops the old `uri_owner` entry and (b) `didClose`s the old uri. The next
   dispatch **reopens at a fresh generation with a fresh uri**. The still-in-transit old publish
   therefore carries a uri no longer in `uri_owner` → dropped in Receive step 1, converting against
   no text and never reaching the version gate at all. Even in the impossible case that it slipped
   past generation, the `document.version` gate (now v+1) would still reject it. Two independent
   rejections.
5. **The stale-result guard is the existing one, unchanged.** `apply_diagnostics_done` stores only
   when `b.document.version == version` and clears `in_flight_version == Some(version)` regardless.
   A user editing during the publish→codeAction window bumps `document.version`, so the assembled
   set arrives, fails the gate, is discarded — the cost is exactly the wasted codeAction fetch,
   never wrong data.
6. **Recovery emissions are version- AND generation-tagged too.** The crash/timeout empties of
   §3.4 carry the tags of the request they abandon, so they can only clear their own in-flight
   slot and only blank a store at the version they were requested for.
7. **Defense-in-depth below all of this** (pre-existing, unchanged): `valid_for` hides any store
   whose `computed_version` drifted; the overlay's `opened_version` + range clamp in
   `diag_apply_selected` (Fix A4) make even a hypothetically wrong range unable to panic or edit
   out of bounds; converter output is bounds-checked against the tagged text (§6).

Residual accepted risk: a server that fabricates positions *within* a live generation+version can
paint a transiently misplaced underline for one debounce cycle — bounded cosmetic, self-healing on
the next edit/recheck, and impossible to convert into data-loss because of (7). No reachable
interleaving of edit / reload / recover / save-as / crash / dict-reload produces a wrong-range
*application*.

### 5.1 The single-in-flight latch invariant (liveness — no permanent wedge)

The above proves *safety* (never wrong). This proves *liveness* (never stuck). The hazard
(round-2 Critical): `in_flight_version` gates both dispatch (`diag_due` requires it `None`) and
wakeups (`diag_deadline` suppresses the deadline while it is `Some`), so a latch that is set but
never cleared **permanently wedges diagnostics for that buffer** — no spin (good) but no checks
ever again (bad). The round-1 design set the latch before an infallible-looking `notify_change`,
but the client thread can die in the window between the `availability()` read and the send, so the
change is neither served nor unwound.

**Invariant:** `in_flight_version == Some(v)` for a buffer ⟹ **at least one terminal
`Msg::DiagnosticsDone` for `(buffer, v)` is guaranteed to arrive** (duplicates are tolerated — e.g.
a late genuine publish after a watchdog-emitted empty terminal for the same `v`; `apply_diagnostics_done`
applies each idempotently under its version/generation filtering, so a second arrival for a version
already cleared merely re-stores the same-or-empty set and re-clears an already-clear latch — no
corruption). It is upheld by two halves that together admit no gap:

- **Dispatch side (§4.3):** the latch is set **only** when `notify_change` returns `Accepted::Yes`
  (the `Cmd::Change` `send` succeeded to a live thread). `Accepted::No` (disconnected send — dead
  thread) sets no latch and shows the degrade hint; a later armed dispatch retries or the provider
  reports `Unavailable`. So a latch is never set without an accepted change.
- **Thread side (§3.2, §3.4):** every accepted change is guaranteed at least one terminal emission
  regardless of how the thread ends — a real publish, the publish/codeAction watchdogs, the
  crash/respawn flush, or the **`FlushGuard` two-part flush that runs even on panic-unwind**
  (`catch_unwind` around the pump). The round-3 refinement closes the "accepted-but-unrecorded"
  gap: because `send(Ok)` proves only that the `Cmd::Change` *entered the channel*, the guard
  **drains `cmd_rx` on exit** and emits a terminal for every unprocessed queued change, in addition
  to flushing the versions the pump already recorded — and the pump records the awaiting slot as the
  first, non-IO step of handling a change. So no matter *where* the thread died relative to reading
  the channel — before receiving, mid-processing, after a child-spawn error, or under panic — an
  accepted change's terminal is emitted. Thread-*spawn* failure never produces an accepted change
  (`ensure_running` does not latch `started` on a spawn `Err`, §3.1).

Because `apply_diagnostics_done` clears `in_flight_version == Some(v)` on *any* arrival for `v`
(even an empty one), the guaranteed terminal emission always unwinds the latch. No reachable path —
including the accepted-but-unrecorded, spawn-failure, and reload/recover-`Cmd::Close` paths — sets
the latch without a matching unwinding event; diagnostics cannot wedge.

---

## 6. Position and suggestion conversion (`lsp_rpc.rs`)

### 6.1 UTF-16 → byte

harper-ls emits UTF-16 code-unit positions and negotiates nothing (grounded constraint), so the
converter is unconditional:

```rust
/// Map an LSP position (0-based line, UTF-16 code-unit column) to a byte offset into `text`.
/// Lines split on '\n' (we sent the text; wordcartel buffers are '\n'-normalized).
/// Per the LSP spec, a column past the line end clamps to the line end; a column landing
/// INSIDE a code point's UTF-16 width maps to that code point's start (never splits a char).
/// Returns None when `line` exceeds the text's line count (diagnostic dropped by the caller).
pub fn utf16_pos_to_byte(text: &str, line: u32, character: u32) -> Option<usize>

/// Half-open byte range for an LSP range; None if either end is unmappable or end < start.
pub fn lsp_range_to_bytes(text: &str, range: &lsp_types::Range) -> Option<std::ops::Range<usize>>
```

Implementation walks the target line's `char_indices()`, accumulating `ch.len_utf16()` until the
column is reached — `O(line length)`, cold path, no allocation. The returned offsets are char
boundaries by construction (the clamp-to-code-point-start rule), so downstream `buffer.slice`
and `build_range_replace` can never hit a boundary panic.

House-convention multibyte tests (é = 1 UTF-16 unit / 2 bytes, 中 = 1 / 3, 🙂 = 2 / 4) are
mandatory (§15), including: a column inside 🙂's surrogate pair, a past-EOL clamp, a past-EOF
line → None, and CRLF-free `\n` handling on the last line without a trailing newline.

### 6.2 `TextEdit` → `Suggestion`

Given a diagnostic with byte range `d` and an action's single `TextEdit` with converted byte
range `e` and `new_text`:

- `e == d` and `new_text.is_empty()` → `Suggestion::Remove`
- `e == d` → `Suggestion::ReplaceWith(new_text)`
- `e.is_empty() && e.start == d.end` → `Suggestion::InsertAfter(new_text)`
- anything else (disjoint edit, partial overlap, multi-edit action) → drop the action.

This is intentionally the exact inverse of how `diag_apply_selected` materializes the three
variants via `build_range_replace` (`ReplaceWith → a..b`, `InsertAfter → b..b`,
`Remove → a..b ""`), so a round-trip through the overlay reproduces the server's intended edit.
Dropping the long tail (multi-edit actions) is safe: the diagnostic still paints and the overlay
still offers ignore/add — no data path depends on suggestion completeness.

### 6.3 Classification (LSP diagnostic → `DiagnosticKind`)

`classify_lsp(diag: &lsp_types::Diagnostic) -> DiagnosticKind`: if the diagnostic's `code`
(string form) or, failing that, its `source`+`message`, identifies harper's spelling linter
(case-insensitive `contains("spell")` on the code string) → `Spelling`; otherwise → `Grammar`.
Two-variant total mapping — no drop bucket at this layer (the old `classify`'s `None` bucket was
a property of harper-core's raw `LintKind` stream; the server's published set is already
curated). The exact `code` payload harper-ls emits is a plan-time probe (§16 R1); the heuristic
is deliberately resilient to it being a linter name ("SpellCheck"), a kind ("Spelling"), or
absent (→ Grammar, which only affects styling and the grammar gate, both safe defaults).

---

## 7. Semantics preserved: grammar toggle, ignore-once, dictionary

### 7.1 Where each filter now lives

| filter | today (in-process) | after Effort A |
|---|---|---|
| grammar off | post-compute drop of `Grammar`-kind lints inside `core::check` | client-thread drop of `Grammar`-classified diags (§3.3.3) + best-effort server-side linter partition (§7.2) |
| ignore once (session) | `ignore_words` snapshot into `check`, re-check round-trip | apply-time filter in `apply_diagnostics_done` + in-place `retain_unignored` on the overlay action (§4.9) |
| personal dictionary | `ignore_words` snapshot + client-appended file | single writer = our `append_word_to_dict`; client-side suppression via `editor.dictionary`; harper-ls `userDictPath` = the *same file* (read-only from harper's side) + a best-effort `didChangeConfiguration` reload nudge (never a second write) |

Note the honest framing: **today's `grammar: bool` is already post-compute filtering** (`check`
computes all lints, then drops `Grammar` kinds when `!opts.grammar`) — so the client-side kind
gate is not a new semantic, it is the same semantic relocated. The server-side partition below is
a load-shedding optimization layered on top, and the client gate remains the correctness
backstop for anything the partition misses.

### 7.2 The grammar/spelling linter partition (server-side, best-effort)

`DiagnosticsConfig.grammar` maps to harper-ls's per-linter toggles in the settings object:

- **Spelling tier (always on):** `SpellCheck: true`.
- **Grammar/style tier (sent `false` when `grammar = false`; left at server defaults when
  `true`):** the curated `GRAMMAR_LINTERS` const in `harper_ls.rs` — the documented harper-ls
  linter names that produce non-spelling lints: `SentenceCapitalization`, `UnclosedQuotes`,
  `WrongQuotes`, `LongSentences`, `RepeatedWords`, `Spaces`, `Matcher`, `CorrectNumberSuffix`,
  `NumberSuffixCapitalization`, `MultipleSequentialPronouns`, `LinkingVerbs`, `AvoidCurses`,
  `TerminatingConjunctions`, `EllipsisLength`, `DotInitialisms`, `BoringWords`, `ThatWhich`,
  `CapitalizePersonalPronouns`, `AnA`, `SpelledNumbers`, `UseGenitive`.
  This list is curated best-effort against harper-ls 2.x's documented linter set; harper-ls
  ignores unknown config keys, newer unlisted linters simply stay at server defaults, and the
  §7.1 client gate guarantees `grammar = false` shows spelling only regardless of list drift.

Two deliberate behavior deltas, stated for the record:
- **`grammar = true` is richer than before.** The old core `classify` admitted only four
  `LintKind`s (Spelling/Repetition/Grammar/Capitalization) and silently dropped the rest;
  harper-ls's curated default linter surface (dashes, quotes, spacing, style) now flows through,
  classified `Grammar`. This is the intended product of adopting the canonical checker — not a
  regression to engineer around. Users who want less run `grammar = false` (spelling only).
- **`grammar = false` no longer wastes zero server work** — the partition turns the listed
  linters off server-side, but any unlisted linter still computes and is dropped client-side.
  Bounded waste, correct result.

### 7.3 Ignore-once stays client-side and ephemeral (locked F4)

`editor.session_ignores` is untouched as state. Both it and the persistent `editor.dictionary`
(§7.4) feed a single ignore predicate — surface word ∈ `dictionary ∪ session_ignores`,
case-insensitive — consulted in exactly two places:

- `apply_diagnostics_done` — on every landing set (post-version-gate): drop `Spelling`
  diagnostics whose flagged surface (buffer slice of the clamped range, lowercased) is in the
  union (same case-insensitive comparison `check` used). Apply-time filtering means a word ignored
  or dictionary-added *after* a publish was emitted still disappears when that publish lands.
- `retain_unignored(editor)` — the same predicate run over the active store in place, so the
  overlay's "ignore once" and "add to dictionary" (§4.9) remove underlines instantly with no
  server round-trip.

Ignore-once remains ephemeral (`session_ignores`, session-scoped); dictionary words persist via
§7.4's writer. harper-ls's own *ignore* mechanism is never invoked (it persists; our ignore is
deliberately ephemeral — the semantic mismatch behind locked F4).

### 7.4 The dictionary: no-data-loss, belt-and-suspenders

**Hard constraint (Codex round 1): existing users' saved words must never be orphaned or
re-flagged.** The probe verified that harper-ls **does honor `userDictPath` for `untitled:` docs**
(a custom word was not flagged) — *but only once the config PULL is answered* (§8): with no
`workspace/configuration` responder, harper ignores the dictionary entirely. So harper's dictionary
help is real yet contingent on the config responder. We keep our own mechanism as the authoritative,
harper-independent one and let harper be a *reader* of the same file — never a writer of it:

- **Path.** `cfg.diagnostics.dictionary` (default `<config_dir>/wordcartel/dictionary.txt`,
  already ~-expanded/defaulted by `config.rs`) is both (a) loaded at startup into
  `editor.dictionary` (the existing `bounded_read_opt` block — **kept**) and (b) passed to
  harper-ls as `userDictPath` (§8.1) — the same on-disk file, same one-word-per-line format.
- **Suppression is client-side and unconditional.** `editor.dictionary` is unioned with
  `session_ignores` in the apply-time ignore filter (§4.3) and `retain_unignored`, so every saved
  word is suppressed **regardless of whether harper-ls ever reads the file** (e.g. if the config
  pull were mis-answered). This is the no-data-loss guarantee: it depends on no server behavior at
  all.
- **Single writer, no double-write (round-2 #3).** `append_word_to_dict` (**kept, unchanged**) is
  the *sole* writer to `dictionary.txt` on add-to-dict; `editor.dictionary.insert` gives instant
  suppression. We do **not** call harper's appending `HarperAddToUserDict` command (it would write
  the same file a second time). Instead `reload_dictionary()` sends a best-effort
  `didChangeConfiguration` resend so harper re-reads the file *we* wrote. If harper's reload is a
  no-op or its path handling differs, the user still sees the word suppressed (client-side) and
  persisted (our writer) — only the *server-side* suppression is best-effort, and it is invisible
  either way because the client filter hides the word.
- **`dictionary = None`** (no config dir resolvable): the `userDictPath` key is omitted (harper
  uses its own default), add-to-dict shows the existing "no dictionary path configured" status for
  the persist path, and we skip the reload nudge — but still insert into `editor.dictionary` for
  session suppression. Unchanged from today.
- **Migration:** because the path and format are byte-identical to today's, there is nothing to
  migrate. No words are ever written to a harper-default location instead of ours, and never
  written twice.

---

## 8. Configuration: the PULL model (`workspace/configuration` responder + `didChangeConfiguration` trigger)

**Empirically verified (harper-ls 2.1.0 probe) and MUST-FIX (round-3 B):** harper-ls delivers
config by **pulling**, not by consuming the push. It advertises nothing until, on init and per-doc,
it sends `workspace/configuration` **requests** (with an `id`) and waits for the client's response.
**A `didChangeConfiguration` push alone delivers nothing** — in the probe, until the client
answered the pull, harper produced zero diagnostics and ignored `userDictPath`. Therefore config in
Effort A is a *pair*:

1. **The pull responder (§3.3, first-class) — UNWRAPPED response.** The client answers each
   `workspace/configuration` request with a **result array** of **bare, unwrapped** settings
   objects, one per `params.items` entry (verified 2.1.0: the request sends `items = [{}]` with no
   `section`, and the **unwrapped** object is what makes `userDictPath` apply and diagnostics flow):
   ```json
   { "jsonrpc": "2.0", "id": <echoed>, "result": [
       { "dialect": "American",
         "userDictPath": "/home/user/.config/wordcartel/dictionary.txt",
         "maxFileLength": 10000000,
         "linters": { "SpellCheck": true, "SentenceCapitalization": false } }
   ] }
   ```
   This is the load-bearing half: without it, all config silently defaults (American / all
   linters / no dictionary → every custom word flagged).
2. **The push trigger — NESTED object.** We still send `workspace/didChangeConfiguration` after
   `initialized` (and on `reload_dictionary`) with the settings **nested under `"harper-ls"`**;
   harper reacts by **re-pulling** via `workspace/configuration`, which the responder answers with
   the unwrapped shape above. So the push (nested) is only a trigger; the pull response (unwrapped)
   is what actually supplies config. This wrapped-vs-unwrapped asymmetry is deliberate and verified.

The `didChangeConfiguration` push payload (trigger only):

```json
{ "settings": { "harper-ls": {
    "userDictPath": "/home/user/.config/wordcartel/dictionary.txt",
    "maxFileLength": 10000000,
    "linters": { "SpellCheck": true, "SentenceCapitalization": false, ... }
} } }
```

(the `linters` map per §7.2; grammar-tier keys are only included when `grammar = false`. The one
remaining low-risk reconfirm for the plan's packaged-version probe is the `section`/nesting on the
pinned version — if a future harper sends a non-empty `section`, the responder still returns our
object per requested item; §16.)

### 8.1 `userDictPath`

Per §7.4 — the same `dictionary.txt` our own writer owns; harper reads it only, *and only when the
config pull is answered*. `reload_dictionary()` triggers a fresh pull→answer cycle so harper
re-reads the file after our writer appends a word — best-effort, never a second write.

### 8.2 `maxFileLength`

harper-ls silently skips documents over its default 120 KB `maxFileLength` (grounded — and the
publish watchdog §3.4 is the safety net for any skip we fail to predict). We raise it to
`limits::HARPER_MAX_FILE_LENGTH = 10_000_000` so real long-form documents check.

### 8.3 Client-side send cap

Full-document sync means every recheck ships the whole text over stdio. A new
`limits::DIAG_MAX_SEND_BYTES = 8 * 1024 * 1024` (8 MiB — comfortably under the 10 M-char server
limit even for 1-byte-per-char text, and of the same order as the existing `MAX_SESSION_BYTES`
class of shell caps) gates `dispatch_diagnostics` (§4.3): over-cap documents get a one-line
status and **no in-flight state** — nothing to wedge, nothing to time out. This bound is
proportional-to-work discipline, not a correctness need.

---

## 9. Degradation when harper-ls is absent (locked F1)

- Detection is the spawn attempt itself (`ErrorKind::NotFound` → terminal `Unavailable`;
  §3.2) — runtime, per-session, no config.
- The user experience: entering Review renders normally; the first dispatch shows
  `grammar checker unavailable — install harper-ls (Arch: pacman -S harper)` in the status line
  (`INSTALL_HINT` const in `diag_provider.rs`), once per Review entry (`diag_hint_shown` latch,
  §4.3) — informative, not naggy, and never a silent wait. Everything else about the editor —
  including Review itself, which stays a provider-neutral render mode (locked F1 views) — is
  fully functional.
- `Availability::Unavailable` also suppresses the `REVIEW · Harper` attribution (§10), so the
  status line itself communicates "Review without a checker".
- Packaging: `optdepends=('harper: …')` in `packaging/arch/PKGBUILD` (§4.15).

---

## 10. Status attribution: `REVIEW · Harper`

`render_status.rs::status_left_text`'s `mode_text` match arm for `Review` becomes:

```rust
crate::editor::RenderMode::Review =>
    if editor.diag_provider.availability() == crate::diag_provider::Availability::Ready {
        return_mode = format!("REVIEW · {}", editor.diag_provider.name()); // rendered [REVIEW · Harper]
    } else { "REVIEW" }
```

(mechanically: `mode_text` changes from `&'static str` to `Cow<'static, str>`; the surrounding
`format!` calls are unchanged). `Idle`/`Starting`/`Unavailable` all show plain `REVIEW` — the
attribution asserts a *live* provider, which is what makes it meaningful when provider #2 and
`view_harper`/`view_vale` tuples arrive (locked F1 views). This is the entire multi-provider
surface Effort A builds — deliberately minimal.

The per-frame cost is one mutex read behind the `Review` arm only; the other three modes don't
touch the provider.

---

## 11. Build changes

- `wordcartel-core/Cargo.toml`: remove `harper-core`. Core keeps `#![forbid(unsafe_code)]` and
  zero IO — deleting its only heavyweight dependency, the point of H2.
- `wordcartel/Cargo.toml`: add `lsp-types = "0.97"` (typed structs for params where convenient;
  in 0.97 the URI type is `lsp_types::Uri` — a newtype over `fluent_uri::Uri<String>` — with **no**
  `Url::from_file_path`, which is one reason we do not derive URIs from file paths) and
  `serde_json = "1"` (JSON-RPC envelopes ride as `serde_json::Value`; already transitively in
  `Cargo.lock`). **No `url` crate:** URIs are opaque `untitled:wcartel-<id>-<gen>` strings built
  with `format!` (§3.3), so there is no `file://` construction and thus no percent-encoding or
  relative-path concern.
- Expected effects (verify in the effort report): `Cargo.lock` sheds the `harper-*`/`burn-*`
  tree; `lto = "fat"` release builds get materially faster; binary shrinks. No feature flags
  anywhere (locked F6: no dual path).

---

## 12. H18 tail: supply-chain scanning (`cargo deny`)

The effort's final task, run against the post-swap dependency tree:

- Add a workspace-root **`deny.toml`**: `[advisories]` (RustSec CVEs — deny vulnerabilities,
  warn unmaintained), `[licenses]` (allow the permissive set the tree actually uses — MIT,
  Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0 — enumerated from a real `cargo deny check
  licenses` run, not guessed), `[bans]` (`multiple-versions = "warn"`; no denied crates
  initially), `[sources]` (crates.io only; the `repar` path dependency is out of scope for
  source checking by nature).
- Findings from the first real run are triaged in the effort report; version-bump fixes within
  reach are applied, the rest recorded.
- `cargo deny check` is documented (CLAUDE.md hardening section note + the effort report) as a
  release-checklist step. It is **not** added to the merge GATEs — promoting it is a CLAUDE.md
  edit for the human, exactly like the smoke suite's promotion clause. `cargo-audit` is
  subsumed (cargo-deny's advisories check covers the same RustSec DB) and not separately added.
- Backlog: `H18` → shipped by this effort; `H2` closes as answered-by-removal (ledger note).

---

## 13. Resource behavior (proportional-to-work audit)

- **Idle is free:** no Review, or Review with a settled buffer → nothing armed → `next_wake`
  contributes `None`; the client thread blocks on `recv`; harper-ls itself sits idle with no
  traffic. Zero polling anywhere in the new machinery (watchdog deadlines exist only while a
  request is outstanding).
- **Edge-triggered:** every send is caused by an edit (`arm_if_edited`) or an explicit command
  (`recheck_diagnostics`, enter-Review arm) — never wall-clock.
- **Memory:** the client holds one `String` per open-in-server buffer (the last-sent text — the
  price of correct UTF-16 conversion and the staleness argument) + `O(diagnostics)`; released on
  `didClose`/shutdown. The 8 MiB send cap bounds the per-buffer copy.
- **Startup:** net win — the warm thread (which burned a core for ~seconds building the FST
  dictionary in-process at every launch when `enabled`) is gone; launch does zero
  diagnostics work. First-Review latency is the harper-ls handshake + first lint, in a
  subprocess, with the "starting…" status covering it.
- **Threads:** two long-lived (client + reader) replacing today's thread-per-check + warm
  thread; both exit on shutdown/degrade.

---

## 14. Command-surface conformance

**This effort does not add, remove, or change any command, menu row, palette entry, keybinding
hint, or user-settable option.** Statement per the contract:

- `view_review`, `cycle_render_mode`, `quick_fix`, `diag_next`, `diag_prev`,
  `recheck_diagnostics` are untouched (registrations, guards, and `set_render_mode` — the law-6
  shared setter — all unchanged).
- The provider is **not** a user-settable option in Effort A: there is exactly one provider, so
  no set-per-state primitives, no cycle, no persisted setting (law 2 has nothing to bite on —
  no new `SettingsSnapshot` field or config key is introduced). The provider selector and
  `view_harper`/`view_vale` `(Review, provider)` tuples are explicitly provider-#2 work
  (locked decision 3).
- `REVIEW · Harper` is display-only state derived from provider availability — not an option,
  not command-reachable state.
- Existing `diagnostics.*` config keys keep their current, pre-existing surface status
  (startup-seeded, no runtime commands — any curation of that is backlog A3b territory, not
  this effort). `linters` remains parsed-and-unconsumed by commands (dormant until
  provider #2).
- The contract's enforcing tests (palette-completeness, every-option-has-a-command, hint
  re-resolution) continue to pass trivially — this effort adds nothing to their domains.

Verdict: **conformant; effectively N/A — the command surface is not touched.**

---

## 15. Testing strategy

Unit (all CI-safe — nothing below requires harper-ls installed; M3/Fs-seam precedent):

1. **`lsp_rpc`**: frame writer/reader round-trip incl. split reads and back-to-back frames;
   malformed frame → error; `utf16_pos_to_byte`/`lsp_range_to_bytes` with é/中/🙂 (per house
   test conventions), surrogate-interior column, past-EOL clamp, past-EOF `None`, no-trailing-
   newline last line; `TextEdit`→`Suggestion` all four rules of §6.2; `classify_lsp` code
   string/absent-code cases.
2. **`HarperState`** (pure state machine — the payoff of §3.3): scripted sequences asserting
   the returned `Action`s: handshake order (initialize advertising `workspace.configuration=true` →
   initialized → didChangeConfiguration → queued replay); **config PULL responder** — an inbound
   `workspace/configuration` request produces a `Send(result-array)` with one harper settings
   object per `params.items` entry (the round-3 MUST-FIX; a `didChangeConfiguration` alone must
   NOT be relied on for delivery); didOpen-then-didChange versioning + `lsp_version`
   `saturating_add` at `i32::MAX` (H7 stance — no panic, value pins); `doc_uri` opaque form
   (`untitled:wcartel-<id>-<gen>`, identical for saved and unsaved; a save is a plain didChange,
   no reopen); **generation attribution** — a publish whose uri is absent from `uri_owner`
   (superseded generation) is dropped, and a `notify_close`+reopen cycle assigns a strictly
   greater generation and a distinct uri; **`Cmd::Close` emits the terminal before removing state**
   (round-3 #2 — an outstanding awaiting/assembling version yields an empty version-tagged
   `DiagnosticsDone` first); **the reload/recover race (§5 item 4)** — scripted: await outstanding
   for gen g, `notify_close` (asserting the terminal emission), reopen at g+1, then the old-uri
   publish arrives and is dropped (never converted); **omitted `version` accepted via generation**
   (harper-2.1.0 case — `version: None` still lands under the uri tag); codeAction response with the
   **verified `{kind:"quickfix", edit.changes:{uri:[{newText,range}]}}` shape** →
   `Emit(DiagnosticsDone)` with `ReplaceWith(newText)` suggestions attached and sorted, and
   command-only actions (`kind:None`, empty edit) dropped; parked-assembly generation superseded
   mid-codeAction → discarded; empty publish emits immediately; codeAction error/timeout emits
   suggestionless; publish watchdog emits empty tagged set; **the `FlushGuard` two-part flush on
   thread exit** — tracked (awaiting+assembling+queued) AND channel-drain: a `Cmd::Change` still
   sitting unread in `cmd_rx` is drained and emits an empty version-tagged `DiagnosticsDone` (the
   round-3 accepted-but-unrecorded gap), and a panic-injected pump still flushes via `catch_unwind`;
   ServerEof within budget → `Respawn` + docs marked unopened + awaiting flushed + `Restarted`;
   budget exhaustion → `Unavailable` + `Degraded`; shutdown handshake.
3. **`diagnostics_run`**: existing suite carries over (`append_word_to_dict` **retained** — its
   parent-dir test stays); new — **the latch invariant (§5.1)** via a `RecordingProvider` with a
   settable `Accepted` return: `Accepted::Yes` → `dispatch_diagnostics` sets `in_flight_version`;
   **`Accepted::No` → latch stays `None` + hint shown** (the wedge guard); over-cap doc → status +
   no in-flight + no call; `Unavailable` → hint once per Review entry + no in-flight;
   `apply_diagnostics_done` ignore filtering over `dictionary ∪ session_ignores` (case-insensitive,
   Spelling-only) layered on the existing version-gate tests; `retain_unignored`.
4. **`diag_provider`**: `apply_provider_event` both variants — `Restarted` re-arms (asserting the
   threaded `clock` drives `arm(now, debounce_ms)`) gated on `should_run_diagnostics`; `Degraded`
   sets the hint; `NullProvider` inertness. **Delivery under BOTH matches** — `reduce_dispatch`
   routes `DiagProviderEvent` to `apply_provider_event` at rest, and `prompts.rs::intercept`
   delivers it under an open modal (a `Degraded` reaches the status line while a prompt is up — the
   round-3 #3 two-site fix); the sweep-precision check — `DiagProviderEvent` does **not match** the
   `ClipboardPaste`/paste/mouse early-returns in `menu`/`palette`/`theme_picker`/`file_browser`/
   `splash`, so it passes through to `reduce_dispatch`.
5. **`search_ui`**: the ignore/add-dict branches' new bodies — ignore: session insert + instant
   refilter; add-dict: **single write** — `append_word_to_dict` writes once + `editor.dictionary`
   insert + provider `reload_dictionary` recorded (assert no second file write); stale
   `opened_version` still refuses (unchanged guard).
6. **`workspace` + `save`** (with `RecordingProvider`): `close_buffer_now` calls `notify_close(id)`
   in all three shapes — active-removal, inactive-removal, AND replace-last-ordinary (old id
   closed before the slot is overwritten); `reload_from_disk` and `load_recovered` each call
   `notify_close(id)` during the wholesale replacement (the §5-item-4 close+reopen signal).
7. **e2e (`e2e.rs`, TestBackend)**: degradation journey — enter Review with the default
   `NullProvider` forced `Unavailable` via a test provider, assert the install hint in the
   status area and normal editing; attribution journey — provider `Ready` → status shows
   `REVIEW · Harper`.
8. **Integration (dev-machine, `#[ignore]`-gated)**: one real-binary conversation test that
   spawns actual `harper-ls` if on PATH and drives the *full* handshake including **answering the
   `workspace/configuration` pull** (without which harper emits nothing — this test would catch a
   broken responder), then asserts "teh" → a Spelling diagnostic with a `ReplaceWith("the")`
   suggestion lands through the full client, and a `userDictPath` word is not flagged — runs where
   the toolchain has harper, skips cleanly elsewhere.
9. **PTY smoke (advisory, per the CLAUDE.md contract)**: one new check — if `harper-ls` is on
   PATH: open a file containing a misspelling, enter Review, wait past debounce + handshake,
   assert the session stays alive and the status line shows Review (attribution when timing
   allows); absent harper-ls → the check reports SKIP. Mandatory-run, advisory-pass, quoted
   verbatim in the pre-merge report.

---

## 16. Risks

- **R1 — harper-ls behavioral facts.** A real harper-ls **2.1.0 stdio probe** now VERIFIES the
  facts the design leans on; only two small reconfirm items remain for the plan's probe (rerun
  against the *packaged* version the effort targets):
  - **Verified (2.1.0), baked in — fallbacks retired:** `untitled:`+`languageId="markdown"` docs
    lint identically to `file://` (§3.3); `userDictPath` applies to `untitled:` docs (§7.4); the
    config **PULL** model — harper delivers config only via `workspace/configuration` requests, the
    push alone delivers nothing (§8); the pull request sends `items = [{}]` (empty section) and the
    **response must be the UNWRAPPED bare settings object** (`result: [{dialect, userDictPath,
    linters…}]`, not `{"harper-ls":{…}}`) — this exact shape made `userDictPath` apply (§8);
    `version` echo is `None` (generation-in-URI is correctly load-bearing, §Receive step 2); the URI
    is echoed verbatim; codeAction returns structured `{kind:"quickfix",
    edit.changes:{uri:[{newText,range}]}}` with clean `newText` (§3.3.6).
  - **Remaining reconfirm (plan probe, packaged version — low-risk):** (1) that the pull
    request/response `section`/nesting is unchanged on the pinned/packaged harper (if a non-empty
    `section` appears, the responder still answers per requested item); (2) whether harper re-reads
    `userDictPath` on a config resend (only affects best-effort *server-side* dict suppression — the
    client filter hides the word regardless, §7.4).
  - **Still isolated behind resilient fallbacks (unchanged):** diagnostic `code` payload (§6.3 —
    `contains("spell")`, absent → `Grammar`); codeAction `diagnostics` echo (§3.3.6 — unique-overlap
    else drop). Neither can corrupt (§5).

  Net vs round 1: the probe shrank the risk set from six unknowns to two low-risk reconfirms, and
  the config-PULL discovery is folded as a MUST-FIX (§8), not a risk.
- **R2 — full-document sync cost on large docs**: bounded by the 8 MiB cap + Review-only
  debounce; accepted (the server is the canonical implementation; incremental sync doesn't
  exist there).
- **R3 — attribution residual race** (§5 last paragraph): bounded cosmetic, self-healing,
  cannot corrupt.
- **R4 — server never publishes** (over-limit skip, hung server): publish watchdog unwedges the
  single-in-flight pipeline (§3.4); worst case is "no underlines + no hint" for one cycle.
- **R5 — out-of-box regression**: fresh installs without harper get no grammar checking where
  the old build had it embedded. Accepted product decision (locked F1) — mitigated by the
  install hint, `optdepends`, and Review remaining useful as a reading mode.
- **R6 — protocol robustness against a hostile/buggy server**: all parsing is
  `serde_json`-typed with per-message error tolerance (a malformed message is dropped; a
  malformed *frame* is treated as stream corruption → respawn path); positions are
  bounds-checked; the untrusted-input stance matches M2/M4.

---

## 17. Why not harper-cli

Rejected (locked): it is self-described as a debugging tool with no stability contract, its JSON
emits display strings ("Replace with: …") that would need brittle parsing instead of typed
edits, and a one-shot CLI generalizes to nothing — the LSP client is a reusable provider
capability (vale-ls next) aligned with Effort P.

## 18. Out of scope (explicit)

Multi-provider merge/dedup and the provider selector; `view_harper`/`view_vale`; consuming
`DiagnosticsConfig.linters` (dormant until provider #2); an embedded fallback provider; runtime
commands for `diagnostics.*` options (A3b); harper-ls version management/bundling; persisting
ignores.
