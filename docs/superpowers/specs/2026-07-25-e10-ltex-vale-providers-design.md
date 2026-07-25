# E10 — ltex-ls-plus + vale-ls providers, shared LSP-client core, engine menu, JVM lifecycle: design spec

**Date:** 2026-07-25. **Status:** draft for Codex spec gate.
**Item:** E10 (`backlog.toml` id `E10`, "Multi-engine linting (b)"; scope + rulings recorded in
`docs/ux-backlog.md` `<!-- item: E10 -->`, "DECISIONS RULED" block, 2026-07-25).
**Branch:** `effort-e10-ltex-vale-providers` off main. No source edits before plan execution.
**Grounding inputs:** `scratchpad/lens-linting-arc/fable-analysis.md` + `e10-grounding-forks.md`
(all eight forks human-ruled 2026-07-25); every claim below re-verified against the working tree
at HEAD. All anchors are SYMBOL names (file + symbol), never line numbers, except where a specific
expression is quoted.

**Framing intent (human):** two more analysis engines — LanguageTool (via ltex-ls-plus) and Vale
(via vale-ls) — join harper behind the shipped multi-provider diagnostics spine, implemented
*elegantly on top of the code as it is now* (post-SPINE, post-S6/S8): one shared LSP-client core
instead of three drifting clones, zero changes to the dispatch/store/render seams, and a JVM
lifecycle that keeps the resource conventions honest.

---

## 1. Summary and locked decisions

Human-ruled decisions (2026-07-25, from `scratchpad/lens-linting-arc/e10-grounding-forks.md`;
recorded verbatim in the E10 prose):

1. **Warm-up = first-check-special deadline.** A `FIRST_CHECK_TIMEOUT_MS` (~180 s) governs the
   publish watchdog until ltex's first publish for the current child process arrives; then the
   normal watchdog. Keep accepting during warm (the shipped init-queue semantics). Status = a
   single STEADY "warming …" indicator — no blink, no progress bar, no self-driven animation
   (the idle-free law forbids a wall-clock repaint loop). §4, §12.
2. **Architecture = shared LSP-client core.** Extract the engine-generic machine
   (`HarperState` + pump + `FlushGuard`) into a new `lsp_client.rs`, generic over an engine
   spec; **refactor harper onto it FIRST as its own intermediate-green task — harper's existing
   tests pass UNMODIFIED (the pin)** — then add ltex + vale as thin engine specs. One copy of the
   terminal-guarantee / watchdog / warm logic. §3.
3. **Idle-shutdown = keep-warm with a ~15-min idle timeout**, configurable
   (`[diagnostics.ltex] idle_shutdown_min`, `0` = never), **ltex-only** (vale/harper never shut
   down). Mechanism = suspend-the-child (park the thread, kill the JVM; the next accepted change
   rides the existing respawn/queue-replay path; deliberate suspends never consume the respawn
   budget). Armed on the leaving-Review **predicate transition** (`should_run_diagnostics`
   flipping false — which happens at `set_render_mode` AND buffer switches), cleared on re-entry;
   the due timestamp lives in editor state, read by a `timers.rs::SUBSYSTEMS` row. §5, §6.
4. **vale auto-install: NOT wired.** No `installVale`, no install config key; vale-ls is told
   `installVale: false` explicitly. Absence degrades exactly like harper: spawn-Err →
   `Unavailable` + per-engine `INSTALL_HINT`. §8.
5. **Default engine:** harper-first catalog order is the fallback, plus a config-file-only
   `[diagnostics] default_engine` key that overrides the lens seed in `install_core_providers`
   when set AND the named engine is installed+enabled. NO command (config-only, like `language`).
   §13.
6. **ltex posture:** documented optdepends (PKGBUILD entry) + per-engine `INSTALL_HINT` naming
   "requires Java 21+". §7, §14.4.
7. **Engine menu:** a builtin dynamic section under **View** — rows =
   `MenuRowAction::Command(toggle_engine_<e>)` with state-in-label, built by a plain
   `fn(&Editor)` over `ProviderSet::sources()` + `availability()`. Menu ⊆ palette by
   construction. No `MenuRowAction::Plugin`, nothing from the deferred dynamic-menu effort
   (E12-only). A top-level "Analysis" menu is deferred to E8. §11.
8. **Per-engine config (blessed minimal):** `[diagnostics.ltex]` = `language` +
   `idle_shutdown_min`; `[diagnostics.vale]` = nothing (vale self-discovers `.vale.ini`);
   `[diagnostics] default_engine`. NOT: ltex java_path/heap, vale config-path override, any
   install flag. §9.

**Scope boundary:** E10 only — providers + shared-core refactor + engine menu + JVM lifecycle +
the config/command/status surfaces above. The viewing/action delta (`href`/"learn more", detail
region, per-engine dictionary/rule writers, executeCommand relay) is **E11, a separate later
effort** — nothing here consumes `Diagnostic.code`/`href` beyond continuing to populate them.

---

## 2. Grounding: the seams at HEAD (verified; what E10 rides vs touches)

### 2.1 The spine needs ZERO changes for N=3

Verified path for any N (all in `wordcartel/src/diagnostics_run.rs` unless noted):
`arm_enabled` arms every `enabled_sources()`; `DiagStore::due_sources` yields armed,
not-in-flight sources; `dispatch_diagnostics` snapshots the buffer ONCE, then `dispatch_one` per
due source — consumes that source's deadline, `ensure_running(source)`, shows the generic
`"starting {label}…"` on `Availability::Starting`, latches `in_flight_version` per-slot on
`Accepted::Yes`; `Msg::DiagnosticsDone { source, .. }` routes to `slot(source)`;
`apply_provider_event(Restarted)` re-arms only its own source; `retain_unignored` refilters every
slot; `ProviderSet::notify_close_all` / `shutdown_all` iterate all entries. **E10's spine
insertion points are exactly:** the `install_core_providers` catalog append + its exhaustive
provider-construction `match` (compiler-forced arms), the `registry.rs` command siblings
(§10), the two engine specs (§7, §8), and the `lib.rs` module declarations (`mod lsp_client;`,
`mod ltex_ls;`, `mod vale_ls;` — added in T1/T5/T6 respectively). Nothing else in the
dispatch/store/marshal layer changes.

*Cosmetic note (carried, not fixed here):* `dispatch_diagnostics` applies one global
`DIAG_MAX_SEND_BYTES` cap to the shared snapshot with a single "document too large for grammar
checking" status; with engines of differing server-side caps that message is engine-unspecific.
Acceptable; per-engine tightening stays inside each provider (`notify_change` re-checks).

### 2.2 The render layer is ZERO-TOUCH

`render.rs::gather_row_ctx` composes `diag_all` (from `diagnostics_run::active_lens_diags`),
the S8 prose lens (`lenses::active_pos_matches`), search, selection, and block paint in one
`RowCtx`; S6 ventilate participates via the shared per-row content-byte maps
(`ventilate::resolve` consulted in the same fn). Ventilate has no `RenderMode` gate — Review ×
ventilate compose today, and byte-ranged diagnostics map through ventilated rows by construction
(providers convert LSP ranges against the exact sent snapshot: `lsp_rpc::lsp_range_to_bytes`
against `DocState.text`, never the live buffer). **E10 changes nothing in
`render.rs`/`derive.rs`/`ventilate.rs`/`lenses.rs`.** The only view-adjacent edits are the status
segment (§12) and the menu section (§11).

### 2.3 The decouple-from-E8 discipline

E10 keys on the delegating predicate pair `should_run_diagnostics` / `should_show_diagnostics`
(`diagnostics_run.rs`) and on `set_render_mode`'s existing predicate-gated arm — **never on
`RenderMode::Review` literals**. Verified at HEAD that every shipped gate already routes through
the pair (`timers::diag_deadline`, `active_lens_diags`, `arm_if_edited`,
`apply_provider_event`). E10 adds engine VALUES, no new toggle shapes; if E8 later redefines
"what summons analysis," the ltex lifecycle moves with the predicate.

### 2.4 The harper template facts the design leans on

- **Init-queue semantics:** during `Phase::Initializing`, inbound `Cmd`s queue with NO watchdog
  deadline (`ClientState`-to-be: `on_inbound` pre-Running → `queued.push`; deadlines are stamped
  only when `on_change` runs at replay in `on_initialized`). `Accepted::Yes` is returned (send
  reached a live thread), the latch holds, `FlushGuard` covers death.
- **The landmine:** `PUBLISH_TIMEOUT_MS = 10_000` (`harper_ls.rs`) stamps every post-Running
  check; ltex's first check takes 30 s–2 min (JVM + LanguageTool model load, landing in
  first-CHECK latency, after `initialize` completes) → a cloned-unchanged watchdog would emit a
  false-empty terminal mid-warm. §4 fixes this in the core.
- **One-way start latch:** `HarperLs::ensure_running` takes `rx: Option<Receiver>` and latches
  `started` — a fully shut-down provider cannot restart. Harmless today (shutdown only at app
  exit); §5's suspend mechanism deliberately parks the THREAD and kills only the CHILD, so the
  latch never needs reopening.
- **Respawn machinery:** `on_server_gone` → flush + `Action::Respawn` (budget
  `MAX_SPAWN_ATTEMPTS = 3`) → pump respawns → `on_spawned` re-initializes → queue replay. §5
  reuses this as the resume path.
- **Settings delivery differs per engine** (scan `docs/design/prose-linters-scan.md` §B):
  harper = `workspace/configuration` PULL (bare unwrapped objects per `params.items`) +
  `didChangeConfiguration` push-as-re-pull-trigger; ltex = PULL + the custom
  `ltex/workspaceSpecificConfiguration` server→client request; vale-ls = `initializationOptions`
  only (no config exchange; vale reads `.vale.ini` itself). This is the engine-spec hook (§3.2).

---

## 3. The shared LSP-client core (`wordcartel/src/lsp_client.rs`)

### 3.1 What moves, what stays

**Moves to `lsp_client.rs`, genericized over `E: LspEngine`** (mechanical extraction of the
current `harper_ls.rs` items, renamed only where harper-specific):

- `ClientState<E>` (today `HarperState`): `Phase`, `DocState`, `AwaitPublish`, `Assembly`,
  `PendingKind`, `Cmd`, `Inbound`, `Action`, `next_deadline`, `on_spawned`, `on_inbound`,
  `apply_cmd`, `on_change`, `on_close`, `on_server` (+ request/notification/response routing),
  `on_initialized`, `on_publish`, `on_codeaction_response`, `convert_diagnostics`,
  `on_deadline`, `on_server_gone`, `flush_outstanding`, plus the §4 warm field and §5 suspend
  states. Every `DiagSource::Harper` literal in an `Emit` becomes `E::SOURCE`.
- `LspProvider<E>` (today `HarperLs`): the app-side `DiagnosticsProvider` handle, `Shared`,
  `FlushGuard`, `run_client`, `spawn_session` (spawn command from `E::spawn_command()`; thread
  names from `E::CLIENT_THREAD`/`E::READER_THREAD`), `spawn_reader`, `pump` (+ §5's
  `Option<(Child, ChildStdin)>` restructure), `run_actions`, `wait_inbound`, `merge_deadline`.

**Stays in `harper_ls.rs`:** `HarperEngine` (the spec impl), `INSTALL_HINT`,
`PUBLISH_TIMEOUT_MS`/`CODEACTION_TIMEOUT_MS`/`CRASHED_HINT` (the consts the inline tests
reference by name — `HarperEngine`'s trait consts cite them), `GRAMMAR_LINTERS`, the harper
settings builder + `initialize_request` params + `classify_lsp` (as the spec's hook impls), the
type aliases, the re-exports, and the **entire existing 29-test inline module, byte-for-byte
unmodified through T1** (T4's one mechanical field-add is scoped in §9). The complete
symbol-by-symbol census is §3.3's table — the T1 gate rests on it.

`FlushGuard` moves as generic `FlushGuard<E>` (its `state` is `ClientState<E>`); the harper
alias covers the tests' struct-literal construction. The tests' `st.settings_object()` calls
are preserved by a **concrete `impl lsp_client::ClientState<HarperEngine>` block in
`harper_ls.rs`** providing `pub(crate) fn settings_object(&self) -> Value` (delegating to the
same harper settings builder `HarperEngine::answer_request` uses) — a local-type impl, no trait
pollution.

### 3.2 The engine spec

```rust
/// One LSP engine's identity + protocol variations. ZST impls (`HarperEngine`, `LtexEngine`,
/// `ValeEngine`); the core monomorphizes per engine — each `LspProvider<E>` is its own
/// `DiagnosticsProvider`.
pub(crate) trait LspEngine: std::fmt::Debug + Send + 'static {
    const SOURCE: DiagSource;
    const INSTALL_HINT: &'static str;
    /// Shown when the crash-respawn budget is exhausted (generic `on_server_gone` emits
    /// `Degraded(E::CRASHED_HINT)` — today a harper-specific literal in that path).
    const CRASHED_HINT: &'static str;
    const LANGUAGE_ID: &'static str;            // "markdown" for all three
    const CLIENT_THREAD: &'static str;          // e.g. "wcartel-ltex-client"
    const READER_THREAD: &'static str;
    const PUBLISH_TIMEOUT_MS: u64;              // steady-state publish watchdog
    const FIRST_CHECK_TIMEOUT_MS: Option<u64>;  // Some(180_000) for ltex; None = no warm phase
    const CODEACTION_TIMEOUT_MS: u64;
    const SUSPENDABLE: bool;                    // true only for ltex (§5)
    fn spawn_command() -> std::process::Command;
    /// The `initialize` request params (capabilities + initializationOptions).
    fn initialize_params(cfg: &ProviderConfig) -> serde_json::Value;
    /// The `didChangeConfiguration` payload — `None` = this engine never pushes (vale).
    fn settings_push(cfg: &ProviderConfig) -> Option<serde_json::Value>;
    /// Engine-specific server→client REQUESTS (config PULL et al). `Some(result)` = respond
    /// with this result payload; `None` = fall through to the generic handling
    /// (workDoneProgress/create + registerCapability → null result; else -32601).
    fn answer_request(method: &str, req: &serde_json::Value, cfg: &ProviderConfig)
        -> Option<serde_json::Value>;
    fn classify(d: &serde_json::Value) -> DiagnosticKind;
}
```

Generic `on_server_request` order: engine hook first (`answer_request`), then the generic
workDoneProgress/registerCapability null-responses, then -32601 — preserving harper's current
behavior when `HarperEngine::answer_request` handles exactly `workspace/configuration`.

**Engine-specific-branch sweep (Critical-6 completeness check, verified against source):** the
harper-specific values threaded through the generic paths are exactly — `DiagSource::Harper` in
every `Emit` (→ `E::SOURCE`); `CRASHED_HINT` in `on_server_gone`'s budget-exhaustion arm (→
`E::CRASHED_HINT`); `INSTALL_HINT` in the pump's spawn-failure `Degraded` (→ `E::INSTALL_HINT`);
`"harper-ls"` in `spawn_session` (→ `E::spawn_command()`); `"markdown"` in `on_change`'s didOpen
(→ `E::LANGUAGE_ID`); the thread names (→ `E::CLIENT_THREAD`/`E::READER_THREAD`); the settings
push/PULL payloads (→ `settings_push`/`answer_request`); `classify_lsp` in
`convert_diagnostics` (→ `E::classify`); the three timeout consts (→ the trait consts). The
`doc_uri` `"untitled:wcartel-{id}-{gen}"` scheme stays SHARED deliberately: each provider owns
its own server process and `uri_owner` map, so identical URI text across engines cannot collide.

### 3.3 The Task-1 pin, stated precisely — with the COMPLETE census

Task 1 is a **behavior-preserving extraction**: after it, `cargo test` is green with the harper
inline test module **textually unmodified**, and every external reference
(`crate::harper_ls::INSTALL_HINT` ×3 — `diag_provider.rs` tests, `prompts.rs`, `app.rs`;
`crate::harper_ls::HarperLs::new` in `install_core_providers`) still compiles.

**Pin scope, precisely:** byte-for-byte through T1. T4 (config) later makes ONE mechanical,
reviewed edit to the harper test module — adding `language: None` to `ProviderConfig` literals —
scoped and listed in §9. Nothing else in the effort touches the module.

**The census** — every symbol the test module reaches (via `use super::*`, the module's own
`use wordcartel_core::diagnostics::Suggestion`, or direct field access), swept from the full
module text (`harper_ls.rs::tests`):

| Symbol as the tests use it | T1 disposition | Visibility after T1 |
|---|---|---|
| `HarperState::new` + methods `.on_spawned` `.on_inbound` `.on_deadline` `.flush_outstanding` `.next_deadline` | alias `pub(crate) type HarperState = lsp_client::ClientState<HarperEngine>` | methods `pub(crate)` (as today) |
| `st.settings_object()` | concrete `impl ClientState<HarperEngine>` block in `harper_ls.rs` (§3.1) | `pub(crate)` |
| `st.phase = Phase::…` (write); `st.docs.get_mut(..)`; `st.assembling.get/contains_key` | `ClientState` fields move | **ALL `ClientState` fields `pub(crate)`** |
| `Phase::{Initializing, Running}` (named directly) | moves | `pub(crate)` enum; **re-export** `pub(crate) use lsp_client::Phase;` |
| `DocState.{lsp_version, open, generation}` (nested pokes) | moves | all `DocState` fields `pub(crate)` |
| `Assembly.diags` (nested read) | moves | all `Assembly` (and `AwaitPublish`, uniformity) fields `pub(crate)` |
| `Cmd::{Change, Close}`, `Inbound::{Cmd, Server, ServerEof}`, `Action::{Send, Emit, SetAvailability, Respawn, Exit}` | move | `pub(crate)`; **re-exports** in `harper_ls.rs` |
| `FlushGuard { state, cmd_rx, msg_tx }` (struct literal) + `guard.state.…` | moves as `FlushGuard<E>` | alias `pub(crate) type FlushGuard = lsp_client::FlushGuard<HarperEngine>`; its 3 fields `pub(crate)` |
| `HarperLs::new` + `source`/`availability`/`notify_change` | alias `pub type HarperLs = lsp_client::LspProvider<HarperEngine>` | as today |
| `p.rx = None` (handle-field write) | `LspProvider` fields move | **`LspProvider.rx` `pub(crate)`** (sole handle field the tests touch — swept; others stay private) |
| `classify_lsp(&json)` (free fn, called directly) | **STAYS** in `harper_ls.rs`; `HarperEngine::classify` delegates to it | as today |
| `PUBLISH_TIMEOUT_MS`, `CODEACTION_TIMEOUT_MS`, `CRASHED_HINT` (named in assertions) | **STAY** as `harper_ls.rs` consts; `HarperEngine`'s trait consts cite them | module-private (same module as the tests) |
| `INSTALL_HINT` | stays (3 external refs) | `pub` (unchanged) |
| `DIAG_MAX_SEND_BYTES` (over-cap test) | the top-level `use crate::limits::DIAG_MAX_SEND_BYTES;` in `harper_ls.rs` is RETAINED (as `pub(crate) use` if otherwise unreferenced, keeping the build warning-free) | — |
| `ProviderConfig { grammar, dictionary, max_file_length }` literals (in `cfg()` + the settings test) | type unchanged in T1 | — (T4 adds the field, §9) |
| `Msg`, `ProviderEvent`, `Availability`, `Accepted`, `BufferId`, `DiagSource`, `Diagnostic`, `DiagnosticKind`, `Value`, `json!` | `harper_ls.rs`'s existing top-level `use`s retained | — |
| `Suggestion` | test module's own `use` (untouched) | — |

The plan MUST carry this table into T1's step list; the T1 reviewer checks it exhaustively
(compile failure catches an omission, but the table is what makes the check a diff review
rather than an archaeology dig).

Task 1 adds **no behavior and no new tests** (TDD exemption, reason: pure extraction; the
mutation-catch is the pin itself plus the whole-suite gate). All engine-generic NEW behavior
(§4, §5) lands in later tasks TDD-first against a `#[cfg(test)]` `TestEngine` ZST in
`lsp_client.rs` (spec-configurable consts — the `RecordingProvider` precedent at the state level).

---

## 4. The warm-up state machine (first-check-special deadline)

New `ClientState` field: `first_publish_seen: bool` — **reset to `false` in `on_spawned`**
(i.e. per child process: initial spawn, crash respawn, AND §5 resume all re-enter the warm
phase, per the ruling "resets on respawn").

- `on_change` stamps the publish watchdog as
  `now + if !self.first_publish_seen { E::FIRST_CHECK_TIMEOUT_MS.unwrap_or(E::PUBLISH_TIMEOUT_MS) } else { E::PUBLISH_TIMEOUT_MS }`.
- `on_publish` sets `first_publish_seen = true` on any publish attributed to an owned URI
  (before the empty/non-empty branch — an empty first result still proves the engine is warm).
- `on_deadline`, the latch, `Accepted` semantics, and the queue-replay are **unchanged** — the
  warm phase is purely a deadline-selection change. A mid-warm edit cannot re-dispatch (the
  per-slot `in_flight_version` gate in `due_sources`), so at most one check per buffer rides the
  long deadline; `DocState`/generation supersession handles the replay ordering as today.
- Harper and vale set `FIRST_CHECK_TIMEOUT_MS = None` → behavior identical to HEAD (the pin
  covers harper; vale needs no warm phase — Go CLI chain, near-instant).
- ltex sets `Some(180_000)` (the scan's 2-min worst case + margin). `PUBLISH_TIMEOUT_MS` for
  ltex: `15_000` (LanguageTool re-checks are slower than harper's; 10 s is harper-calibrated).

Option B (don't-dispatch-until-Ready) is **rejected** per the ruling: the warm lands after
`initialize` completes (post-`Ready`), so gating dispatch on `Ready` would not dodge the
landmine and would add a Ready-event mechanism for nothing.

**No self-driven animation:** the warm indicator is render-derived state (§12), not a repaint
loop; the pump's `recv_timeout` deadline during warm is the (long) publish watchdog — the app's
run loop stays blocked unless something else wakes it (idle-free law upheld).

---

## 5. Suspend/resume (the idle-shutdown mechanism, provider side)

Core additions (engine-generic; exercised only when `E::SUSPENDABLE`):

- `Cmd::Suspend`, `Phase::Suspended`, `Action::Park`, `Action::Unpark`.
- **Suspend** (`apply_cmd`; meaningful only in `Running` — in any other phase it is a no-op,
  and `Cmd::Suspend` must be added to `on_inbound`'s pre-Running NON-queue set alongside
  `Cmd::Shutdown`-style special handling: a suspend arriving while `Initializing`/`Suspended`
  is simply dropped, never queued — replaying a stale suspend after a resume handshake would
  re-kill the fresh child): emit `self.flush_outstanding()` first (ordinarily empty — 15 idle
  minutes exceed every watchdog — but the flush makes the terminal-guarantee unconditional),
  then `Action::Send(shutdown request)` + `Action::Send(exit notification)` + `Action::Park`;
  `phase = Phase::Suspended`. The suspend shutdown request is **fire-and-forget: NO
  `PendingKind` is registered** — if the server's response arrives before the kill, it routes
  to `on_server_response` with no pending entry → `Vec::new()` (the existing unknown-id arm).
- **Expected-EOF rule (the deliberate-kill hole, closed).** Park kills the child; the reader
  thread then delivers `Inbound::ServerEof`. The EOF routing in `on_inbound` becomes:
  `Inbound::ServerEof => if self.phase == Phase::Suspended { Vec::new() } else
  { self.on_server_gone(now) }` — a deliberate suspend's EOF is DRAINED: no flush (already
  done), no respawn, no budget consumption, no `Restarted` event. No transient "suspending"
  phase is needed: `phase` is set to `Suspended` synchronously in `apply_cmd`, strictly before
  the pump executes Park, and the reader's EOF arrives via the channel strictly after.
- **Park** (pump): kill + wait the child, drop stdin, `SetAvailability(Idle)` — the thread
  parks on `recv` (a blocked thread is free; the JVM was the cost). The pump's child ownership
  restructures to `Option<(Child, ChildStdin)>`; `Action::Send` with no child is dropped under a
  `debug_assert!`.
- **Shutdown-while-Suspended rule (the app-exit hole, closed).** `on_inbound` applies
  `Cmd::Shutdown` in every phase; with no child there is nothing to hand a shutdown request to.
  `apply_cmd`'s Shutdown arm becomes phase-aware: in `Phase::Suspended` it returns
  `vec![Action::Exit]` directly (child already dead, outstanding already flushed at suspend
  time) — no `Send`, no grace. With the expected-EOF rule above, these two rules make
  "`Action::Send` while parked" genuinely unreachable, which is what licenses the
  `debug_assert!` (the §5-draft claim of "unreachable by construction" holds only WITH both
  rules — they are the construction).
- **Resume:** `on_inbound` in `Phase::Suspended` queues the cmd (the existing pre-Running queue
  arm already matches, since `Suspended != Running`) and additionally returns `Action::Unpark`
  for a queued `Cmd::Change` (only Change warrants a JVM). The pump's Unpark = `spawn_session` +
  `on_spawned` — the existing respawn path verbatim: re-`initialize`, `Starting`, queue replay,
  docs re-opened, `first_publish_seen` reset (§4) so the re-warm gets the long deadline.
  **`spawn_attempts` is NOT incremented on Unpark** (deliberate suspends never consume the
  crash-respawn budget — the ruling; `on_spawned` does not touch the counter today, so this
  holds by not adding one). A failed resume spawn follows the existing initial-spawn-failure
  path (`Unavailable` + `Degraded(INSTALL_HINT)`).
- **Trait seam:** `DiagnosticsProvider` gains `fn suspend(&mut self) {}` — a **defaulted**
  method (no impl change required for real non-LSP future providers).
  `LspProvider::<E>::suspend` sends `Cmd::Suspend` iff `E::SUSPENDABLE && self.started`.
  `ProviderSet::suspend_all_idle_heavy()` (name per plan) delegates to every entry — only ltex's
  impl acts. This is the one `DiagnosticsProvider` trait change in the effort.
  **Test observability (not "no edits"):** `diag_provider.rs`'s `ProviderCall` gains a
  `Suspend` variant and `RecordingProvider` overrides `suspend()` to record it — otherwise
  T8's fire-path tests cannot observe the defaulted no-op. Real providers keep the default.

Post-suspend semantics ride the shipped machinery unchanged: availability `Idle` means
`dispatch_one` proceeds; `notify_change` → `Accepted::Yes` (thread alive) → the change queues,
Unpark fires, the handshake replays it, and the publish deadline is stamped at replay time
(§2.4's init-queue semantics — no watchdog runs against a parked engine).

---

## 6. Idle-shutdown, app side (editor state + timers row + transition seam)

- **Editor state:** `Editor.diag_idle_due: Option<u64>` (declared beside `diag_hint_shown`,
  initialized `None`). Not per-buffer, not persisted.
- **Arming — the predicate TRANSITION, not a call site.** The reduce-exit chokepoint that
  already calls `diagnostics_run::arm_if_edited` (`app.rs::reduce` calls it after the extracted
  interceptor-chain body, so it wraps every normal reduce exit — interceptor early-returns and
  the match tail; the ONE bypass is the debug-only `WCARTEL_SMOKE_PANIC` branch, which panics
  by design and is not a normal exit, so the chokepoint claim stands) additionally snapshots
  `summoned_before = should_run_diagnostics(editor)` before dispatch and calls a new
  `diagnostics_run::idle_shutdown_track(editor, summoned_before, clock)` after:
  - `true → false` (left Review by mode change OR buffer switch): if
    `diag_providers.is_enabled(DiagSource::LTeX)` AND `ltex_idle_shutdown_min > 0`, arm
    `diag_idle_due = Some(now + min·60_000)`.
  - `false → true` (re-entered): `diag_idle_due = None` (the grace: any Review re-entry within
    the window cancels; brief buffer flips cost nothing).
  - No transition: no-op. (Edge-triggered by a real state change — the resource law.)
  **No started-ness accessor is added** (the "is it running?" fork, resolved): `started` is
  private to the handle and `Availability::Idle` cannot distinguish never-started from parked —
  so the arm gate deliberately uses ENABLEMENT only, and correctness rests on the
  provider-side guards (`LspProvider::suspend` no-ops unless `E::SUSPENDABLE && started`; a
  send to a dead thread is a discarded `let _`). Cost: at most one spurious loop wake per
  leaving-Review with an enabled-but-never-summoned ltex — and in practice entering Review
  dispatches and `ensure_running`s every enabled engine first, so the never-started case is a
  corner, not a path.
- **Timers row:** `TimedSubsystem { name: "diag_idle", deadline: diag_idle_deadline }` appended
  to `timers::SUBSYSTEMS`, where `diag_idle_deadline(e, _now) = e.diag_idle_due` — the
  `pos_sweep_deadline` shape (gated `Option`, `None` ⇒ never wakes the loop; idle-free upheld).
- **Fire:** in the run loop's timed tick beside `dispatch_diagnostics`:
  `diagnostics_run::diag_idle_fire(editor, now)` — if `diag_idle_due` is reached, clear it and
  call `editor.diag_providers.suspend_all_idle_heavy()`. One-shot (cleared on fire), no re-arm
  until the next leaving-Review transition.

**Startup:** `ensure_running` remains lazy-on-dispatch (unchanged) — the JVM never starts until
Review first summons ltex, exactly the E7 cost-lands-in-the-summoned-view principle.

---

## 7. The ltex engine spec (`wordcartel/src/ltex_ls.rs`)

`LtexEngine`, ~all constants + four small hook fns:

- `SOURCE = DiagSource::LTeX` (reserved vocabulary already in core, verified);
  `LANGUAGE_ID = "markdown"`; threads `"wcartel-ltex-client"`/`"wcartel-ltex-read"`.
- `spawn_command()`: `Command::new("ltex-ls-plus")` with stdio pipes (same shape as harper's
  `spawn_session`; the binary speaks stdio LSP by default — **verify the exact invocation flags
  against the real binary in the live probe, §15 T10**).
- `INSTALL_HINT`: `"language checker unavailable — install ltex-ls-plus (requires Java 21+)"`
  (ruling 6's copy). `CRASHED_HINT`: `"language checker stopped after repeated restarts"`.
- Timeouts: `PUBLISH_TIMEOUT_MS = 15_000`, `FIRST_CHECK_TIMEOUT_MS = Some(180_000)`,
  `CODEACTION_TIMEOUT_MS = 5_000`. `SUSPENDABLE = true`.
- `initialize_params`: harper-shaped capabilities (`workspace.configuration: true`,
  `publishDiagnostics.versionSupport: true`, `codeAction`), `initializationOptions: null`.
- `settings_push(cfg)`: `Some(json!({"ltex": {"language": <cfg.language or "en-US">}}))` —
  the push-as-re-pull-trigger, nested under the engine's section (harper's
  `didchangeconfiguration_push` pattern).
- `answer_request`: `"workspace/configuration"` AND `"ltex/workspaceSpecificConfiguration"` both
  → a result array of **bare, unwrapped** `{"language": …}` settings objects, one per
  `params.items` entry (harper's `answer_configuration` MUST-FIX shape, reused). **T11 live-probe
  update (ltex-ls-plus 18.7.0, single-doc config):** the standard `workspace/configuration` PULL is
  what the real server sends, and our bare per-item response was accepted with diagnostics flowing
  (`MORFOLOGIK_RULE_EN_US`). The custom `ltex/workspaceSpecificConfiguration` extension was NOT
  observed being exercised under the tested configuration — the arm is **handled defensively**
  (served identically to the standard method), not a proven-exercised path. Kept for robustness
  against ltex versions/configs that do send it.
- `classify(d)`: `code` starting with/containing `MORFOLOGIK`, `HUNSPELL`, or `SPELLER` →
  `Spelling`; else fall through to the shared harper heuristic (substring "spell" across
  code/source/message) → else `Grammar`. (LanguageTool spelling rule ids are
  MORFOLOGIK_RULE_* / *_SPELLER_RULE; `code` = ruleId per the scan.)
- Construction (`install_core_providers` arm): `ProviderConfig { grammar:
  cfg.diagnostics.grammar, dictionary: None, max_file_length: HARPER-equivalent unused (see
  §9 note), language: Some(cfg.diagnostics.ltex.language.clone()) }`. The client-side
  `!grammar ⇒ drop Grammar-kind` gate in `convert_diagnostics` is generic and applies — with
  `grammar = false` ltex yields spelling only, the config's documented cross-engine meaning.

The `grammar`-linters push table (`GRAMMAR_LINTERS`) is harper-only and stays in
`HarperEngine::settings_*`; ltex needs no analog (the client-side kind gate is the backstop).

---

## 8. The vale engine spec (`wordcartel/src/vale_ls.rs`)

`ValeEngine` — the near-free third arm:

- `SOURCE = DiagSource::Vale`; `LANGUAGE_ID = "markdown"`; threads
  `"wcartel-vale-client"`/`"wcartel-vale-read"`.
- `spawn_command()`: `Command::new("vale-ls")` (stdio LSP; it shells to the `vale` CLI per
  check — two OS processes in *its* chain, still ONE child from our pump's perspective;
  `spawn_session` shape unchanged).
- `INSTALL_HINT`: `"style linter unavailable — install vale and vale-ls"`.
  `CRASHED_HINT`: `"style linter stopped after repeated restarts"`.
- Timeouts: `PUBLISH_TIMEOUT_MS = 10_000`, `FIRST_CHECK_TIMEOUT_MS = None`,
  `CODEACTION_TIMEOUT_MS = 5_000`. `SUSPENDABLE = false`.
- `initialize_params`: standard capabilities; `initializationOptions:
  {"installVale": false, "syncOnStartup": false}` — ruling 4 made mechanical: vale-ls must
  never self-install or hit the network on our behalf (**key names verified in the live
  probe**; if a key is unrecognized it is ignored — LSP init options are freeform).
- `settings_push`: `None` (vale-ls takes no config exchange; it reads `.vale.ini` via its own
  discovery from the checked file's path — which is why `notify_change`'s `path` matters not:
  our URIs are opaque `untitled:` synthetics, so vale falls back to its default config
  discovery. **Honest limitation, stated:** style resolution follows vale's cwd-relative
  discovery, not the buffer's directory; acceptable for E10, revisit if users report it).
- `answer_request`: always `None` (generic handling suffices; vale-ls's hover/completion fire
  only for `.vale.ini`/style files — the scan's trap — and we never request them).
- `classify(d)`: check name (`code`) containing `"Spelling"` → `Spelling`; else the shared
  heuristic; else `Grammar`.
- Construction: `ProviderConfig { grammar: cfg.diagnostics.grammar, dictionary: None,
  max_file_length: <unused>, language: None }`.

---

## 9. Config surface

`config.rs` — extending the shipped per-engine pattern (`RawDiagnostics.harper:
RawHarperEngine` exists at HEAD; SPINE Task 8):

- `RawDiagnostics` gains `default_engine: Option<String>`, `ltex: RawLtexEngine`
  (`{ language: Option<String>, idle_shutdown_min: Option<u64> }`). **No `RawValeEngine`**
  (ruling 8: vale has no keys — adding an empty table invites drift).
- Cooked `DiagnosticsConfig` gains `default_engine: Option<DiagSource>` (validated at fold:
  unknown name → a config warning naming the known set, the `linters` unknown-engine pattern;
  known name → the enum), `ltex_language: String` (default `"en-US"`),
  `ltex_idle_shutdown_min: u64` (default `15`; `0` = never suspend).
- `ProviderConfig` (diag_provider.rs) gains `language: Option<String>` — the one engine-varying
  knob that must reach a worker. `grammar`/`dictionary`/`max_file_length` stay as-is (harper
  consumes all three; ltex/vale receive inert values — a documented, bounded untidiness
  preferred over per-engine config generics; revisit only if a fourth engine needs more).
  `install_core_providers`'s `linters` unknown-name warning text updates from
  `"(known: harper)"` to the full catalog.

  **The pin's real boundary (Important-9, resolved honestly):** adding the field breaks every
  `ProviderConfig` struct literal, including the harper test module's. The T1 pin is
  byte-for-byte **through T1** (which does not touch `ProviderConfig`); **T4 makes the one
  mechanical, reviewed edit** — adding `language: None,` explicitly (no `Default` derive, no
  `..Default::default()` — house explicitness, and the exhaustive literal keeps the compiler
  forcing future field placements) — at the complete literal census: `harper_ls.rs::tests`
  `cfg()` helper + the `settings_object_omits_dict…` literal; `diag_provider.rs::tests` ×2
  (the `ProviderCall::Configure` pair); production `install_core_providers` (which sets
  `Some(..)` for ltex per §7). The `#[cfg(test)] impl PartialEq for ProviderConfig` in
  `diag_provider.rs` gains the `language` comparison in the same task. No other literal exists
  (swept crate-wide).

---

## 10. Commands + command-surface-contract conformance (explicit statement)

New registrations in `registry.rs`, beside the existing harper siblings (the comment there —
"the ltex/vale effort adds its siblings here" — is this task):

- `analysis_engine_ltex` ("Analysis Engine: LTeX"), `analysis_engine_vale`
  ("Analysis Engine: vale") — **palette-only set-per-state primitives** (menu `None`), each a
  thin delegation to the ONE shared setter `Editor::set_analysis_source` (law 6; the setter
  already refuses disabled engines with an honest status).
- `toggle_engine_ltex` ("Toggle LTeX Engine"), `toggle_engine_vale` ("Toggle vale Engine") —
  **palette-only 2-state toggles** delegating to the shared
  `diagnostics_run::set_engine_enabled` (law 6; law 8's 2-state-toggle form).
- `analysis_next` — **unchanged**: it already cycles `enabled_sources()` generically with
  state-in-label (`MenuMark::Value(active_analysis_source.label())`), the law-8 cycle
  representative in View.

Contract conformance: every new user-visible state change is a command; each derives its palette
row + live keymap hint from the registry for free; menu ⊆ palette holds (§11's rows dispatch
registered commands); the palette-completeness / every-option-has-a-command invariant tests
extend with the four new ids; `default_engine` and the ltex table keys are **config-file-only
values consumed at startup** (like `language`-class settings), not runtime-settable options — no
command obligation. **No contract amendment.**

---

## 11. The engine-management menu section

- New `pub fn engine_menu_rows(editor: &Editor) -> Vec<(String, MenuRowAction)>` in
  `diagnostics_run.rs` (domain module — the `workspace::documents_menu_rows` precedent): for
  each `editor.diag_providers.sources()`, label `format!("{} — {}", src.label(), state)` where
  state = `"off"` when `!is_enabled(src)`, else by `availability(src)`:
  `Unavailable → "not installed"`, `Starting → "warming…"`, `Idle | Ready → "on"`. Action =
  `MenuRowAction::Command(CommandId(<toggle_engine_* for src>))` via an exhaustive match;
  `DiagSource::Plugin(_)` rows are skipped (no command exists — plugin rows are E12's).
- `menu.rs::DYNAMIC_SECTIONS` gains
  `DynamicSection { category: MenuCategory::View, rows: crate::diagnostics_run::engine_menu_rows }`
  — a second entry in the existing fn-pointer table; the build loop already appends dynamic rows
  to the category's group. No `MenuCategory` variant, no `MENU_ORDER` change (top-level
  "Analysis" deferred to E8), no `MenuRowAction` widening.
- *Honest display note:* availability is lazily discovered — an absent binary reads `"on"`
  (`Idle`) until Review first attempts a spawn, then `"not installed"`. Acceptable: probing
  binaries at menu-open would violate the no-work-at-rest posture.

---

## 12. Status: the steady warming indicator

**Current behavior, stated exactly** (`render_status.rs`, the Review arm): the mode segment is
`"REVIEW · {label}"` **only when** `diag_providers.availability(lens) ==
Some(Availability::Ready)`; `Idle`, `Starting`, `Unavailable`, and a missing entry (`None`)
all render plain `"REVIEW"` — the label "asserts a working checker," per the shipped comment.

**Change:** add ONE arm — `Some(Availability::Starting)` renders
`"REVIEW · warming {label}…"`. This ADDS a label to a state that today shows plain `REVIEW`
(not a modification of the Ready label). The full matrix after E10:
`Ready → "REVIEW · {label}"`; `Starting → "REVIEW · warming {label}…"`;
`Idle`/`Unavailable`/`None` → `"REVIEW"` (unchanged). Render-derived (recomputed per frame from
model state — the B18 "status reads the model directly" precedent), therefore STEADY,
self-clearing on the Starting→Ready flip, and incapable of animating (no timer, no repaint
loop — the idle-free law; the steady-warming ruling holds). `dispatch_one`'s existing one-shot
`"starting {label}…"` Info message stays as the transient acknowledgment; the segment is the
steady truth. (The existing `status_line_shows_review_label` test covers the empty-`ProviderSet`
case and survives unmodified; the pre-E10 `status_line_attributes_review_only_when_provider_ready`
test — which asserts the Ready case AND that Starting renders plain `REVIEW` — is REPLACED by T9's
`status_line_review_attribution_matrix` covering the new Ready / Starting-warming / plain-`REVIEW`
matrix.)

---

## 13. Default engine seed

`install_core_providers` (diagnostics_run.rs): the catalog becomes
`&[DiagSource::Harper, DiagSource::LTeX, DiagSource::Vale]` (cycle order = harper-first
fallback). After the existing first-enabled-source seed, apply the override:

```rust
if let Some(want) = cfg.diagnostics.default_engine {
    if editor.diag_providers.is_enabled(want) { editor.active_analysis_source = want; }
    else { warns.push(format!("config: diagnostics.default_engine — \"{}\" is not enabled; using {}",
        want.config_name(), editor.active_analysis_source.label())); }
}
```

Direct field write, matching the shipped seed's own comment ("construction — not
`set_analysis_source`, which would status-message"). Unknown *names* were already rejected at
config fold (§9) — this branch only handles known-but-disabled. Grounded value: the active
engine is NOT persisted (`settings.rs` has no analysis snapshot field — verified), so without
this key a primarily-ltex user re-switches every session.

---

## 14. What E10 does NOT do (boundaries, restated as invariants)

1. **Render zero-touch** (§2.2): no edits to `render.rs`, `derive.rs`, `ventilate.rs`,
   `lenses.rs`, `RowCtx`, or any paint path. The two new engines are new VALUES flowing through
   shipped seams.
2. **E11 boundary:** `Diagnostic.code`/`href` continue to be populated (the generic
   `convert_diagnostics` carries the extraction) and remain unconsumed. No detail region, no
   "learn more", no per-engine dictionary writers, no executeCommand relay, no `DiagOverlay`
   changes.
3. **E8 discipline** (§2.3): no new toggle shapes; lifecycle keys on `should_run_diagnostics`;
   no `RenderMode::Review` literals in new code.
4. **Packaging/docs:** `packaging/arch/PKGBUILD` optdepends gains
   `'ltex-ls-plus: LanguageTool grammar/language diagnostics in Review mode (requires Java 21+)'`
   and `'vale: prose style diagnostics in Review mode (with vale-ls)'` + `'vale-ls: …'`
   (exact package names verified at plan time against the AUR/repos), beside the existing
   harper entry.
5. **No network side effects:** vale-ls is pinned `installVale: false` (§8); nothing downloads
   anything, ever.

---

## 15. Task decomposition (ordered; each task ends green)

**T1 — Extract the shared core (THE PIN TASK; intermediate-green).** Create `lsp_client.rs`
(+ its `lib.rs` `mod` line; generic `ClientState<E>`/`LspProvider<E>`/`FlushGuard<E>`/pump per
§3.1, visibilities per §3.3's census table — the table IS the task's checklist); rewrite
`harper_ls.rs` as `HarperEngine` + consts + the `settings_object` concrete-impl block + aliases
+ re-exports. **Gate: full `cargo test` green with the harper test module and every external
caller textually unmodified; workspace clippy clean.** No new behavior, no new tests (§3.3
exemption). This task lands BEFORE any ltex/vale code exists.

**T2 — Warm-phase deadline (core, TDD).** `first_publish_seen` + `FIRST_CHECK_TIMEOUT_MS`
selection (§4), tested against a `TestEngine` with `Some(small)`: first check gets the long
deadline; post-first-publish checks get the normal one; reset on respawn re-enters the warm
phase; a `None` engine (harper-shaped) is byte-identical to HEAD behavior.

**T3 — Suspend/resume (core, TDD).** `Cmd::Suspend`/`Phase::Suspended`/`Action::Park`/
`Action::Unpark` + pump `Option<(Child, ChildStdin)>` + `DiagnosticsProvider::suspend`
(defaulted) + `ProviderCall::Suspend` + the `RecordingProvider::suspend` override +
`ProviderSet::suspend_all_idle_heavy` (§5). Tests: suspend flushes-then-parks (fire-and-forget
shutdown — no `PendingKind` registered; a late shutdown response is a no-op);
**`ServerEof` in `Suspended` is drained — no flush, no respawn, no budget decrement, no
`Restarted`**; **`Cmd::Shutdown` in `Suspended` → `Action::Exit` directly, no `Send`**; a
suspend arriving in a non-`Running` phase is dropped, never queued; queued Change in
`Suspended` yields Unpark + replay; `spawn_attempts` untouched by Unpark; a non-`SUSPENDABLE`
provider's `suspend()` sends nothing (observed via `RecordingProvider`).

**T4 — Config keys (TDD).** §9's raw structs, folds, defaults, validation warnings
(`default_engine` unknown-name; `linters` known-set text), `ProviderConfig.language` — including
the §9-scoped mechanical `language: None,` additions to the five existing literals (the pin's
one sanctioned test-module edit) + the `#[cfg(test)]` `PartialEq` field.

**T5 — ltex provider (TDD).** `ltex_ls.rs` per §7 + the `install_core_providers` catalog append
+ match arm (compiler-forced). State-machine tests via the spec impl: settings push/PULL shapes
(both methods), classifier table, timeout constants wired.

**T6 — vale provider (TDD).** `vale_ls.rs` per §8 + catalog arm. Tests: initializationOptions
pin (`installVale:false` present in the initialize request), no settings push, classifier.

**T7 — Commands + invariant tests (TDD).** §10's four registrations + the registration/dispatch
tests extending the shipped harper-sibling test block; palette-completeness inventory updates.

**T8 — Idle-shutdown app side (TDD).** §6: `diag_idle_due`, `idle_shutdown_track` at the
reduce-exit chokepoint (transition table tested: mode-change out, buffer-switch out, re-entry
clear, disabled/zero-config never arms), the `SUBSYSTEMS` row (`next_wake` unaffected when
unarmed — the idle-free guardrail test, `pos_sweep` precedent), `diag_idle_fire` →
`suspend_all_idle_heavy` (RecordingProvider observes; ltex-only via `SUSPENDABLE`).

**T9 — Engine menu + warming status (TDD).** §11 rows fn (state labels per
enabled×availability matrix; Plugin rows skipped) + `DYNAMIC_SECTIONS` entry + a menu-build
test; §12 render_status branch + a status test (Starting → "warming", Ready → plain label).

**T10 — Default-engine seed (TDD) + packaging.** §13 override + tests (set+enabled wins;
known-but-disabled warns and falls back; absent key = HEAD behavior); PKGBUILD optdepends
(§14.4).

**T11 — LIVE PROBE (mandatory-run, advisory-pass).** Drive the real `ltex-ls-plus` and
`vale-ls` binaries through a real Review session (the `tui-interact` harness): handshake,
first-check warm + steady "warming" segment, publish → underlines, idle-suspend → JVM gone
(process table) → re-entry resummon, vale `.vale.ini` pickup, absent-binary hints. Quote results
verbatim in the pre-merge report; a machine without the binaries records SKIP. This is the A21
lesson institutionalized: synthetic `Inbound::Server` tests validate OUR machine, never the real
servers' framing/flags — the probe validates the empirical claims flagged in §7/§8
(spawn flags, `ltex/workspaceSpecificConfiguration` shape, initializationOptions key names).

Dependencies: T1 → {T2, T3} → T5; T4 → {T5, T6, T8, T10}; T5/T6 → T7 → T9; T3+T4 → T8;
T10/T11 last. Sizes: T1 large (mechanical but wide); T2/T3 medium (the delicate cores);
the rest small.

---

## 16. Risks + honest flags

1. **T1 is the effort's real risk concentration** — a wide mechanical refactor of shipped,
   delicately-tested code. Mitigations: the unmodified-tests pin, the aliases/re-export
   strategy (§3.3), landing it alone and first, and the Codex plan gate's known hard check on
   intermediate-greenness.
2. **Empirical protocol claims** (marked in §7/§8): ltex spawn flags, the custom config
   request's response shape, vale-ls initializationOptions key names. Each is pinned by T11's
   live probe; none is load-bearing for the core design (all live in engine-spec hook impls —
   worst case is a 5-line spec fix, not a machine change).
3. **`pub(crate)` widenings** — `ClientState` (all fields), `DocState`/`Assembly`/
   `AwaitPublish` (nested fields), `FlushGuard` (3 fields), `LspProvider.rx` — each pin-forced,
   enumerated in §3.3's census, none leaving the crate.
4. **ltex first-check inside `initialize` instead:** if a given ltex build front-loads its warm
   into the handshake, the shipped init-queue semantics already cover it (no deadline runs
   pre-Running) — the design is safe on BOTH sides of where the warm lands.
5. **vale `.vale.ini` discovery vs opaque URIs** (§8) — stated limitation, revisit on demand.
6. **Suspend with work outstanding** — handled unconditionally by the flush-first rule (§5);
   in practice unreachable (watchdogs ≪ idle timeout).

## 17. Merge gates (per CLAUDE.md)

`cargo test` green (all suites); `cargo build`/`test --no-run` warning-free; workspace clippy
clean (`too_many_lines`/module budgets respected — the T1 extraction SHRINKS `harper_ls.rs`,
and `lsp_client.rs` inherits the state-machine's existing fn granularity); PTY smoke suite run
+ one-line summary quoted (advisory); T11 probe results quoted (advisory); the
command-surface invariant tests green with the new ids.
