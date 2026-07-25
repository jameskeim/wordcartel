# E10 — ltex-ls-plus + vale-ls Providers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two new LSP diagnostics providers (ltex-ls-plus, vale-ls) behind the shipped
multi-provider spine, built on a shared LSP-client core extracted from `harper_ls.rs`, plus the
ltex JVM lifecycle (first-check warm deadline + idle suspend), the engine-management menu
section, per-engine config, and the four command siblings.

**Architecture:** Spec `docs/superpowers/specs/2026-07-25-e10-ltex-vale-providers-design.md`
(committed a79e1cb — READ the sections each task cites). T1 extracts the engine-generic protocol
machine (`ClientState<E>`/`LspProvider<E>`/`FlushGuard<E>`) into `wordcartel/src/lsp_client.rs`
behind an `LspEngine` trait, pinned by harper's 29 inline tests passing byte-for-byte unmodified.
T2/T3 add the warm-deadline and suspend/resume machinery to the core (TDD against a `TestEngine`
ZST). T4–T10 add config, the two engine specs, commands, idle-shutdown app wiring, menu/status,
and the default-engine seed. T11 is the live-binary probe.

**Tech Stack:** Rust (edition per workspace), serde_json, std::process/mpsc/thread. No new
dependencies.

## Global Constraints

Every task's requirements implicitly include ALL of these (values verbatim from the spec):

- **Timeouts:** ltex `PUBLISH_TIMEOUT_MS = 15_000`, `FIRST_CHECK_TIMEOUT_MS = Some(180_000)`;
  vale `PUBLISH_TIMEOUT_MS = 10_000`, `FIRST_CHECK_TIMEOUT_MS = None`; harper keeps
  `PUBLISH_TIMEOUT_MS = 10_000`, `FIRST_CHECK_TIMEOUT_MS = None`; `CODEACTION_TIMEOUT_MS = 5_000`
  for all three.
- **Idle-shutdown:** ltex-only (`SUSPENDABLE = true` for ltex, `false` for harper/vale); config
  `[diagnostics.ltex] idle_shutdown_min`, default `15`, `0` = never; arm on the
  `should_run_diagnostics` true→false transition, clear on false→true.
- **Command ids (palette-only, no menu entry):** `analysis_engine_ltex`, `analysis_engine_vale`
  (→ `Editor::set_analysis_source`), `toggle_engine_ltex`, `toggle_engine_vale`
  (→ `diagnostics_run::set_engine_enabled`). `analysis_next` is NOT modified. Command-surface
  contract: conformant as-is, **no amendment** (spec §10).
- **Config keys:** `[diagnostics] default_engine` (config-only, no command);
  `[diagnostics.ltex] language` (default `"en-US"`) + `idle_shutdown_min`; **no
  `[diagnostics.vale]` table**; no java_path/heap/install keys.
- **vale-ls:** `initializationOptions {"installVale": false, "syncOnStartup": false}`; no
  auto-install, ever.
- **Menu:** engine rows under `MenuCategory::View` via a new `DYNAMIC_SECTIONS` entry; rows are
  `MenuRowAction::Command(toggle_engine_<e>)` with state-in-label; `DiagSource::Plugin(_)` rows
  skipped.
- **ZERO-TOUCH:** `render.rs`, `derive.rs`, `ventilate.rs`, `lenses.rs`, `RowCtx` are never
  edited. `render_status.rs` gets exactly the §12 `Starting` arm; nothing else in the view layer.
- **Decouple-from-E8:** lifecycle code keys on `should_run_diagnostics` /
  `should_show_diagnostics`; **never write a `RenderMode::Review` literal** in new code.
- **E11 boundary:** `Diagnostic.code`/`href` keep being populated (the generic
  `convert_diagnostics` carries the extraction); nothing consumes them.
- **THE T1 PIN:** harper's inline test module (`harper_ls.rs::tests`, 29 tests) passes
  **byte-for-byte unmodified through T1**. The sole sanctioned later edit is T4's mechanical
  `language: None,` field-add at the five `ProviderConfig` literal sites. Do not "improve",
  reformat, or rename anything the census (T1) lists as kept/re-exported.
- **Every commit** leaves `cargo test --workspace` green and `cargo clippy --workspace
  --all-targets` clean. Do NOT run `cargo fmt` (hand-formatted repo). House style: match
  neighbors, `—` in prose comments, no emoji.
- Commit messages end with the project trailers (CLAUDE.md commit rules): the
  `Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>` line, then a `Claude-Session:` line
  with YOUR harness-supplied session URL (read it from your own harness instructions — never
  construct or invent it).

**Dependency edges:** T1 → {T2, T3} → T5; T4 → {T5, T6, T8, T10}; T5/T6 → T7 → T9; T3+T4 → T8;
T10/T11 last.

---

### Task 1: Extract the shared LSP-client core (THE PIN TASK)

**Files:**
- Create: `wordcartel/src/lsp_client.rs`
- Modify: `wordcartel/src/harper_ls.rs` (production half only — the `mod tests` block is
  UNTOUCHABLE), `wordcartel/src/lib.rs` (one `mod` line)
- Test: NONE NEW — **pin, do not manufacture a red.** This is a behavior-preserving extraction;
  the verification IS the unmodified 29-test harper module + the full suite (spec §3.3 TDD
  exemption). Any impulse to "add a quick test" here is out of scope.

**Interfaces (produced — later tasks rely on these exact names):**
- `lsp_client::LspEngine` (trait, below), `lsp_client::ClientState<E>`,
  `lsp_client::LspProvider<E>` (with `pub fn new(msg_tx: Sender<Msg>, cfg: ProviderConfig) -> Self`),
  `lsp_client::FlushGuard<E>`, `lsp_client::{Phase, Cmd, Inbound, Action}` — all `pub(crate)`
  except `LspProvider` (`pub`, matching today's `pub struct HarperLs`).
- `harper_ls::HarperEngine`, aliases `harper_ls::HarperState` / `harper_ls::HarperLs` /
  `harper_ls::FlushGuard`, and `harper_ls.rs`'s re-exports (census below).

**The census (spec §3.3) — this is the task's literal checklist.** Every row must hold when you
finish; the test module reaches these via `use super::*` or direct field access:

| Symbol as tests use it | Disposition | Visibility |
|---|---|---|
| `HarperState::new`, `.on_spawned`, `.on_inbound`, `.on_deadline`, `.flush_outstanding`, `.next_deadline` | alias to `ClientState<HarperEngine>` | methods `pub(crate)` |
| `st.settings_object()` | concrete `impl ClientState<HarperEngine>` block in harper_ls.rs | `pub(crate)` |
| `st.phase` (write), `st.docs` (get_mut), `st.assembling` (get/contains_key) | `ClientState` fields | ALL fields `pub(crate)` |
| `Phase::{Initializing, Running}` | moved | `pub(crate)` + re-export |
| `DocState.{lsp_version, open, generation}` | moved | all fields `pub(crate)` |
| `Assembly.diags` (+ `AwaitPublish`, uniformity) | moved | all fields `pub(crate)` |
| `Cmd::{Change, Close}`, `Inbound::{Cmd, Server, ServerEof}`, `Action::{Send, Emit, SetAvailability, Respawn, Exit}` | moved | `pub(crate)` + re-export |
| `FlushGuard { state, cmd_rx, msg_tx }` literal, `guard.state` | moved as `FlushGuard<E>` | alias + 3 fields `pub(crate)` |
| `HarperLs::new`, `.source/.availability/.notify_change`, `p.rx = None` | alias to `LspProvider<HarperEngine>` | `rx` field `pub(crate)`; other handle fields stay private |
| `classify_lsp` (free fn, called directly) | STAYS in harper_ls.rs (original body) | as today |
| `PUBLISH_TIMEOUT_MS`, `CODEACTION_TIMEOUT_MS`, `CRASHED_HINT` | STAY as harper_ls.rs consts | module-private |
| `INSTALL_HINT` | stays | `pub` |
| `DIAG_MAX_SEND_BYTES` | `pub(crate) use crate::limits::DIAG_MAX_SEND_BYTES;` | re-export |
| `Msg`, `ProviderEvent`, `Availability`, `Accepted`, `ProviderConfig`, `BufferId`, `DiagSource`, `Diagnostic`, `DiagnosticKind` | `pub(crate) use` re-exports (test-only consumers must not leave the non-test build with unused-import warnings) | re-export |
| `Value`, `json!` | keep `use serde_json::{json, Value};` (used by remaining production code) | — |
| `Suggestion` | test module's own `use` — untouched | — |
| `ProviderConfig { grammar, dictionary, max_file_length }` literals | type UNCHANGED in this task | — |

External references that must keep compiling unmodified: `crate::harper_ls::INSTALL_HINT`
(diag_provider.rs tests, prompts.rs, app.rs) and `crate::harper_ls::HarperLs::new`
(diagnostics_run.rs `install_core_providers`).

- [ ] **Step 1: Add the module line**

In `wordcartel/src/lib.rs`, next to the existing `mod harper_ls;` / `mod lsp_rpc;` lines, add:

```rust
mod lsp_client;
```

- [ ] **Step 2: Create `wordcartel/src/lsp_client.rs` — trait + moved machine**

Open `harper_ls.rs` in one pane. The file below is built by **cut-and-paste of the existing
code** (do NOT retype bodies — move them) plus the substitution table in Step 3. Layout of the
new file:

```rust
//! The engine-generic LSP client core (E10 T1): the pure `ClientState<E>` protocol state
//! machine, the `LspProvider<E>` app-side handle, the pump/reader threads, and the
//! `FlushGuard<E>` terminal-guarantee latch — extracted verbatim from `harper_ls.rs` and
//! parameterized over [`LspEngine`]. Engine identity/protocol variation lives ONLY in the
//! trait; one copy of the terminal-guarantee/watchdog logic serves every engine
//! (spec 2026-07-25-e10 §3).
use std::collections::HashMap;
use std::io::BufReader;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::app::Msg;
use crate::diag_provider::{Accepted, Availability, DiagnosticsProvider, ProviderConfig, ProviderEvent};
use crate::editor::BufferId;
use crate::limits::DIAG_MAX_SEND_BYTES;
use wordcartel_core::diagnostics::{Diagnostic, DiagnosticKind, DiagSource};

/// One LSP engine's identity + protocol variations (spec §3.2). ZST impls
/// (`HarperEngine`, `LtexEngine`, `ValeEngine`); the core monomorphizes per engine.
pub(crate) trait LspEngine: std::fmt::Debug + Send + 'static {
    const SOURCE: DiagSource;
    const INSTALL_HINT: &'static str;
    /// Shown when the crash-respawn budget is exhausted (`on_server_gone`).
    const CRASHED_HINT: &'static str;
    const LANGUAGE_ID: &'static str;
    const CLIENT_THREAD: &'static str;
    const READER_THREAD: &'static str;
    /// Steady-state publish watchdog (per-check `DiagnosticsDone` guarantee).
    const PUBLISH_TIMEOUT_MS: u64;
    /// `Some(ms)` = warm-phase deadline until the first publish of each child process
    /// (consumed by T2; declared here so the trait is complete from day one).
    const FIRST_CHECK_TIMEOUT_MS: Option<u64>;
    const CODEACTION_TIMEOUT_MS: u64;
    /// Idle suspend-the-child eligibility (consumed by T3; ltex-only).
    const SUSPENDABLE: bool;
    fn spawn_command() -> Command;
    /// The `initialize` request `params` object (capabilities + initializationOptions).
    fn initialize_params(cfg: &ProviderConfig) -> Value;
    /// The `didChangeConfiguration` `settings` payload — `None` = this engine never pushes.
    fn settings_push(cfg: &ProviderConfig) -> Option<Value>;
    /// Engine-specific server→client REQUESTS. `Some(result)` = respond with this `result`
    /// payload; `None` = generic handling (workDoneProgress/registerCapability → null; else
    /// -32601 method-not-found).
    fn answer_request(method: &str, req: &Value, cfg: &ProviderConfig) -> Option<Value>;
    fn classify(d: &Value) -> DiagnosticKind;
}
```

Then move, in order, from `harper_ls.rs` (everything between the consts and the test module):
`Cmd`, `Inbound`, `Action`, `Phase`, `DocState`, `AwaitPublish`, `Assembly`, `PendingKind`,
the state struct (renamed `ClientState<E: LspEngine>` with a
`_engine: std::marker::PhantomData<E>` field appended and initialized in `new`), its full
`impl<E: LspEngine> ClientState<E>` block, `pos`, `raw_envelope`, `codeaction_request`,
`Shared`, the handle struct (renamed `pub struct LspProvider<E: LspEngine>` with the same
PhantomData), its `new`/`set_availability` impl, the
`impl<E: LspEngine> DiagnosticsProvider for LspProvider<E>` block, `FlushGuard<E>` + its `Drop`,
`run_client`, `spawn_session`, `spawn_reader`, `Control`, `pump`, `exit_notification`,
`set_availability`, `write_frame_to`, `run_actions`, `merge_deadline`, `Wait`, `wait_inbound`.
`classify_lsp` does NOT move (stays in harper_ls.rs). Mark every struct's fields and every enum
`pub(crate)` per the census (yes, ALL `ClientState`/`DocState`/`AwaitPublish`/`Assembly`/
`FlushGuard` fields; on `LspProvider` only `rx` — the rest stay private).

- [ ] **Step 3: Apply the substitution table to the moved bodies**

Every occurrence, no exceptions (grep the new file afterward to prove each old form is gone):

| Old text (in moved code) | New text |
|---|---|
| `DiagSource::Harper` | `E::SOURCE` |
| `PUBLISH_TIMEOUT_MS` | `E::PUBLISH_TIMEOUT_MS` |
| `CODEACTION_TIMEOUT_MS` | `E::CODEACTION_TIMEOUT_MS` |
| `CRASHED_HINT.into()` | `E::CRASHED_HINT.into()` |
| `INSTALL_HINT.into()` (pump spawn-failure) | `E::INSTALL_HINT.into()` |
| `Command::new("harper-ls").arg("--stdio")` (in `spawn_session`) | `E::spawn_command()` |
| `.name("wcartel-harper-client".into())` (in `ensure_running`) | `.name(E::CLIENT_THREAD.into())` |
| `.name("wcartel-harper-read".into())` (in `spawn_reader`) | `.name(E::READER_THREAD.into())` |
| `"languageId":"markdown"` (didOpen in `on_change`) | `"languageId": E::LANGUAGE_ID` |
| `classify_lsp(d)` (in `convert_diagnostics`) | `E::classify(d)` |
| `HarperState` | `ClientState<E>` (or `Self`) |
| `HarperLs` | `LspProvider<E>` (or `Self`) |

`spawn_session` and `spawn_reader` become generic (`fn spawn_session<E: LspEngine>(…)`,
`fn spawn_reader<E: LspEngine>(…)`) so the consts resolve; `run_client`/`pump` likewise
(`fn pump<E: LspEngine>(guard: &mut FlushGuard<E>, …)`).

Three methods change SHAPE (write these exactly — they are the only non-verbatim bodies):

```rust
    /// The `initialize` request — engine params under the shared envelope.
    fn initialize_request(&self, id: u64) -> Value {
        json!({"jsonrpc":"2.0","id":id,"method":"initialize",
            "params": E::initialize_params(&self.cfg)})
    }

    /// The `didChangeConfiguration` PUSH — engine payload; `None` = engine never pushes
    /// (vale), and the caller skips the frame entirely.
    fn settings_push_action(&self) -> Option<Action> {
        E::settings_push(&self.cfg).map(|settings| Action::Send(json!({
            "jsonrpc":"2.0","method":"workspace/didChangeConfiguration",
            "params":{"settings": settings}})))
    }

    /// Server→client requests: engine hook first, then the generic null-responses, then -32601.
    fn on_server_request(&self, v: &Value) -> Vec<Action> {
        let method = v["method"].as_str().unwrap_or("");
        if let Some(result) = E::answer_request(method, v, &self.cfg) {
            return vec![Action::Send(json!({"jsonrpc":"2.0","id":v["id"].clone(),
                "result": result}))];
        }
        match method {
            "window/workDoneProgress/create" | "client/registerCapability" =>
                vec![Action::Send(json!({"jsonrpc":"2.0","id":v["id"].clone(),"result":Value::Null}))],
            _ => vec![Action::Send(json!({"jsonrpc":"2.0","id":v["id"].clone(),
                "error":{"code":-32601,"message":"method not found"}}))],
        }
    }
```

Consequential edits at their call sites (all inside moved code):
- `answer_configuration` and `settings_object` are DELETED from the generic machine (harper's
  versions live in harper_ls.rs, Step 4).
- `didchangeconfiguration_push()` call sites become `settings_push_action()`:
  in `on_initialized` the push is appended only when `Some` —
  ```rust
        let mut out = vec![
            Action::Send(json!({"jsonrpc":"2.0","method":"initialized","params":{}})),
        ];
        out.extend(self.settings_push_action());
        // Handshake complete → the provider is LIVE (spec §10). …(keep the existing comment)…
        out.push(Action::SetAvailability(Availability::Ready));
  ```
  and in `apply_cmd`: `Cmd::ReloadDict => self.settings_push_action().into_iter().collect(),`
  `Cmd::Configure(cfg) => { self.cfg = cfg; self.settings_push_action().into_iter().collect() }`.
  (Harper always returns `Some`, so its emitted frames — and the pinned
  `["initialized", "workspace/didChangeConfiguration"]` assertion — are unchanged.)

- [ ] **Step 4: Rewrite `harper_ls.rs`'s production half**

Everything above `#[cfg(test)] mod tests` is replaced with (complete):

```rust
//! The harper-ls engine (Effort A; E10 T1): `HarperEngine` — harper's identity, timeouts,
//! settings PULL/push shapes, and classifier — over the engine-generic `lsp_client` core.
//! The protocol state machine, pump, and `FlushGuard` moved to `lsp_client.rs` verbatim
//! (spec 2026-07-25-e10 §3); the inline test module below is the extraction PIN and is
//! byte-for-byte identical to its pre-extraction form.
use serde_json::{json, Value};

// Test-surface re-exports (T1 census): the inline test module reaches these via
// `use super::*`; `pub(crate) use` keeps the non-test build warning-free (a private import
// consumed only by cfg(test) code would trip unused_imports).
pub(crate) use crate::app::Msg;
pub(crate) use crate::diag_provider::{Accepted, Availability, ProviderConfig, ProviderEvent};
pub(crate) use crate::editor::BufferId;
pub(crate) use crate::limits::DIAG_MAX_SEND_BYTES;
pub(crate) use crate::lsp_client::{Action, Cmd, Inbound, Phase};
pub(crate) use wordcartel_core::diagnostics::{Diagnostic, DiagnosticKind, DiagSource};

/// Status hint shown when harper-ls is unavailable (spec §9) — harper's own install copy.
pub const INSTALL_HINT: &str =
    "grammar checker unavailable — install harper-ls (Arch: pacman -S harper)";

/// Publish watchdog: if the server never publishes for a sent version, emit an empty terminal
/// after this so the single-in-flight latch never wedges (spec §3.4).
const PUBLISH_TIMEOUT_MS: u64 = 10_000;
/// codeAction watchdog: emit the converted diagnostics suggestionless if the fix fetch stalls.
const CODEACTION_TIMEOUT_MS: u64 = 5_000;
/// Degrade hint shown once the respawn budget is exhausted (distinct from the not-installed hint).
const CRASHED_HINT: &str = "grammar checker stopped after repeated restarts";

/// Grammar/style linter names toggled off when `grammar = false` (spec §7.2). Curated best-effort;
/// harper ignores unknown keys and the client-side kind gate is the correctness backstop.
const GRAMMAR_LINTERS: &[&str] = &[
    "SentenceCapitalization","UnclosedQuotes","WrongQuotes","LongSentences","RepeatedWords",
    "Spaces","Matcher","CorrectNumberSuffix","NumberSuffixCapitalization","MultipleSequentialPronouns",
    "LinkingVerbs","AvoidCurses","TerminatingConjunctions","EllipsisLength","DotInitialisms",
    "BoringWords","ThatWhich","CapitalizePersonalPronouns","AnA","SpelledNumbers","UseGenitive",
];

/// The harper-ls engine spec (E10 §3.2) — identity + protocol variation only; the machine
/// lives in `lsp_client`.
#[derive(Debug)]
pub(crate) struct HarperEngine;

impl crate::lsp_client::LspEngine for HarperEngine {
    const SOURCE: DiagSource = DiagSource::Harper;
    const INSTALL_HINT: &'static str = INSTALL_HINT;
    const CRASHED_HINT: &'static str = CRASHED_HINT;
    const LANGUAGE_ID: &'static str = "markdown";
    const CLIENT_THREAD: &'static str = "wcartel-harper-client";
    const READER_THREAD: &'static str = "wcartel-harper-read";
    const PUBLISH_TIMEOUT_MS: u64 = PUBLISH_TIMEOUT_MS;
    const FIRST_CHECK_TIMEOUT_MS: Option<u64> = None; // resident + fast — no warm phase
    const CODEACTION_TIMEOUT_MS: u64 = CODEACTION_TIMEOUT_MS;
    const SUSPENDABLE: bool = false;

    fn spawn_command() -> std::process::Command {
        let mut c = std::process::Command::new("harper-ls");
        c.arg("--stdio");
        c
    }

    /// Advertises `workspace.configuration = true` (harper PULLs config from us, spec §8) and
    /// `publishDiagnostics.versionSupport = true` — byte-identical to the pre-T1 request.
    fn initialize_params(_cfg: &ProviderConfig) -> Value {
        json!({
            "processId": std::process::id(),
            "rootUri": Value::Null,
            "clientInfo": {"name":"wordcartel","version": env!("CARGO_PKG_VERSION")},
            "initializationOptions": Value::Null,
            "capabilities": {
                "workspace": {"configuration": true,
                    "didChangeConfiguration": {"dynamicRegistration": false}},
                "textDocument": {
                    "publishDiagnostics": {"versionSupport": true},
                    "codeAction": {"dynamicRegistration": false}
                }
            }
        })
    }

    /// The PUSH is NESTED under `"harper-ls"` (spec §8): only a trigger that makes harper
    /// re-PULL; the unwrapped pull RESPONSE is what actually delivers config.
    fn settings_push(cfg: &ProviderConfig) -> Option<Value> {
        Some(json!({"harper-ls": harper_settings(cfg)}))
    }

    /// The `workspace/configuration` PULL responder: a result array of BARE, unwrapped
    /// settings objects — one per `params.items` entry (spec §8, MUST-FIX shape).
    fn answer_request(method: &str, req: &Value, cfg: &ProviderConfig) -> Option<Value> {
        if method != "workspace/configuration" { return None; }
        let items = req["params"]["items"].as_array().map(|a| a.len()).unwrap_or(1);
        let obj = harper_settings(cfg);
        Some(Value::Array((0..items).map(|_| obj.clone()).collect()))
    }

    fn classify(d: &Value) -> DiagnosticKind { classify_lsp(d) }
}

/// The BARE, unwrapped harper settings object (spec §8) — the pre-T1 `settings_object` body,
/// taking the cfg explicitly so both the engine hooks and the pinned test accessor share it.
fn harper_settings(cfg: &ProviderConfig) -> Value {
    let mut linters = serde_json::Map::new();
    linters.insert("SpellCheck".into(), Value::Bool(true));
    if !cfg.grammar {
        for name in GRAMMAR_LINTERS { linters.insert((*name).into(), Value::Bool(false)); }
    }
    let mut obj = serde_json::Map::new();
    obj.insert("dialect".into(), Value::String("American".into()));
    if let Some(p) = &cfg.dictionary {
        obj.insert("userDictPath".into(), Value::String(p.to_string_lossy().into_owned()));
    }
    obj.insert("maxFileLength".into(), json!(cfg.max_file_length));
    obj.insert("linters".into(), Value::Object(linters));
    Value::Object(obj)
}

/// The pure harper protocol machine — the pre-T1 name, preserved for the pin.
pub(crate) type HarperState = crate::lsp_client::ClientState<HarperEngine>;
/// The app-side harper provider handle — the pre-T1 name (external callers + tests).
pub type HarperLs = crate::lsp_client::LspProvider<HarperEngine>;
/// The harper flush guard — the pre-T1 name (test struct literals).
pub(crate) type FlushGuard = crate::lsp_client::FlushGuard<HarperEngine>;

impl crate::lsp_client::ClientState<HarperEngine> {
    /// Test-visible harper settings for the CURRENT cfg — the pre-T1 method, preserved for
    /// the pin (a concrete inherent impl on the monomorphized type; spec §3.1).
    pub(crate) fn settings_object(&self) -> Value { harper_settings(&self.cfg) }
}
```

…followed by the ORIGINAL `classify_lsp` function, verbatim (its body is unchanged — the
lowercase-"spell" heuristic over code/source/message), and then the UNTOUCHED
`#[cfg(test)] mod tests` block.

- [ ] **Step 5: Verify the pin**

```bash
git diff --stat wordcartel/src/harper_ls.rs   # tests block must show ZERO changed lines
git diff wordcartel/src/harper_ls.rs | grep -A2 "mod tests"   # sanity: no hunk touches it
cargo test -p wordcartel harper_ls:: 2>&1 | tail -3
```
Expected: all 29 harper tests PASS. Then the full gate:
```bash
cargo test --workspace 2>&1 | tail -5
cargo clippy --workspace --all-targets 2>&1 | tail -3
```
Expected: green / clean. If `lsp_client.rs` trips `clippy::too_many_lines` on a moved function,
the function already carried the size before the move — carry over any existing item-local
`#[allow]`; do NOT split moved code in this task.

- [ ] **Step 6: Grep-proof the substitutions**

```bash
grep -n "DiagSource::Harper\|harper-ls\|wcartel-harper" wordcartel/src/lsp_client.rs
```
Expected: NO matches (all engine identity lives behind `E`).

- [ ] **Step 7: Commit**

```bash
git add wordcartel/src/lsp_client.rs wordcartel/src/harper_ls.rs wordcartel/src/lib.rs
git commit -m "refactor: extract engine-generic LSP client core (E10 T1, harper-tests pin)"
```

---

### Task 2: Warm-phase deadline (first-check-special timeout)

**Files:**
- Modify: `wordcartel/src/lsp_client.rs`
- Test: inline `#[cfg(test)] mod tests` in `wordcartel/src/lsp_client.rs` (new module — the
  generic core's own tests, driven through a `TestEngine`)

**Interfaces:**
- Consumes: T1's `ClientState<E>`, `LspEngine::{FIRST_CHECK_TIMEOUT_MS, PUBLISH_TIMEOUT_MS}`.
- Produces: `ClientState.first_publish_seen: bool` (`pub(crate)`, reset in `on_spawned`);
  deadline selection in `on_change`. T5's ltex relies on this machinery verbatim.

- [ ] **Step 1: Write the failing tests**

Add to `lsp_client.rs` (bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A spec-configurable engine for exercising the generic machine (the RecordingProvider
    /// precedent at the state level). Warm phase ON (30 s), suspendable (T3).
    #[derive(Debug)]
    struct TestEngine;
    impl LspEngine for TestEngine {
        const SOURCE: DiagSource = DiagSource::Plugin("test-engine");
        const INSTALL_HINT: &'static str = "test engine unavailable";
        const CRASHED_HINT: &'static str = "test engine crashed";
        const LANGUAGE_ID: &'static str = "markdown";
        const CLIENT_THREAD: &'static str = "wcartel-test-client";
        const READER_THREAD: &'static str = "wcartel-test-read";
        const PUBLISH_TIMEOUT_MS: u64 = 1_000;
        const FIRST_CHECK_TIMEOUT_MS: Option<u64> = Some(30_000);
        const CODEACTION_TIMEOUT_MS: u64 = 500;
        const SUSPENDABLE: bool = true;
        fn spawn_command() -> Command { Command::new("wcartel-no-such-test-engine") }
        fn initialize_params(_cfg: &ProviderConfig) -> Value {
            json!({"processId": Value::Null, "capabilities": {}})
        }
        fn settings_push(_cfg: &ProviderConfig) -> Option<Value> { None }
        fn answer_request(_method: &str, _req: &Value, _cfg: &ProviderConfig) -> Option<Value> { None }
        fn classify(_d: &Value) -> DiagnosticKind { DiagnosticKind::Grammar }
    }

    fn cfg() -> ProviderConfig {
        ProviderConfig { grammar: true, dictionary: None, max_file_length: 10_000 }
    }

    fn sends(acts: &[Action]) -> Vec<&Value> {
        acts.iter().filter_map(|a| if let Action::Send(v) = a { Some(v) } else { None }).collect()
    }
    fn diag_dones(acts: &[Action]) -> Vec<(BufferId, u64)> {
        acts.iter().filter_map(|a| match a {
            Action::Emit(Msg::DiagnosticsDone { buffer_id, version, .. }) =>
                Some((*buffer_id, *version)),
            _ => None,
        }).collect()
    }

    /// Drive `new → on_spawned → initialize response` to a Running machine.
    fn running() -> ClientState<TestEngine> {
        let mut st = ClientState::<TestEngine>::new(cfg());
        let spawn = st.on_spawned(0);
        let id = sends(&spawn)[0]["id"].as_u64().expect("initialize id");
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":{"capabilities":{}}})), 0);
        assert!(!out.is_empty());
        st
    }

    fn change(st: &mut ClientState<TestEngine>, buffer: u64, version: u64, at: u64) {
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(buffer),
            version, path: None, text: "x".into() }), at);
    }
    // (`BufferId` is a `pub u64` newtype — editor.rs — so the literal forms below match the
    // harper tests' `BufferId(0)` exactly.)

    // ── T2: warm-phase deadline ─────────────────────────────────────────────────────────────

    #[test]
    fn first_check_uses_the_long_deadline() {
        let mut st = running();
        change(&mut st, 0, 1, 0);
        assert!(st.on_deadline(TestEngine::PUBLISH_TIMEOUT_MS).is_empty(),
            "normal watchdog must NOT fire during the warm phase");
        let out = st.on_deadline(TestEngine::FIRST_CHECK_TIMEOUT_MS.unwrap());
        assert_eq!(diag_dones(&out), vec![(BufferId(0), 1)],
            "the warm deadline eventually fires an empty terminal");
    }

    #[test]
    fn after_first_publish_the_normal_deadline_applies() {
        let mut st = running();
        change(&mut st, 0, 1, 0);
        // First publish for the owned uri (generation 1) proves the engine warm.
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-0-1","diagnostics":[]}})), 0);
        assert_eq!(diag_dones(&out), vec![(BufferId(0), 1)]);
        // The next check is watchdogged at the NORMAL timeout.
        change(&mut st, 0, 2, 100);
        let fired = st.on_deadline(100 + TestEngine::PUBLISH_TIMEOUT_MS);
        assert_eq!(diag_dones(&fired), vec![(BufferId(0), 2)],
            "post-warm checks use PUBLISH_TIMEOUT_MS");
    }

    #[test]
    fn respawn_re_enters_the_warm_phase() {
        let mut st = running();
        change(&mut st, 0, 1, 0);
        st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-0-1","diagnostics":[]}})), 0); // warm proven
        st.on_inbound(Inbound::ServerEof, 0); // crash → respawn path
        let spawn = st.on_spawned(0);
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":{"capabilities":{}}})), 0);
        change(&mut st, 0, 3, 0);
        assert!(st.on_deadline(TestEngine::PUBLISH_TIMEOUT_MS).is_empty(),
            "on_spawned reset first_publish_seen — the fresh child re-warms (spec §4)");
    }

    #[test]
    fn engine_without_warm_phase_uses_the_normal_deadline_from_the_start() {
        // HarperEngine has FIRST_CHECK_TIMEOUT_MS = None — its own pinned test
        // `publish_watchdog_emits_empty_after_deadline` (harper_ls.rs) covers this; this
        // test guards the unwrap_or fallback generically via the harper alias.
        let mut st = crate::harper_ls::HarperState::new(cfg());
        let spawn = st.on_spawned(0);
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,"result":{}})), 0);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1,
            path: None, text: "x".into() }), 0);
        let out = st.on_deadline(crate::harper_ls::HarperEngine::PUBLISH_TIMEOUT_MS);
        assert_eq!(diag_dones(&out), vec![(BufferId(0), 1)]);
    }
}
```

(`crate::harper_ls::HarperEngine::PUBLISH_TIMEOUT_MS` reaches the TRAIT const — add
`use crate::lsp_client::LspEngine;` inside the test module if the compiler asks for the trait
in scope. `HarperEngine` must be reachable: it is `pub(crate)` per T1.)

- [ ] **Step 2: Run to verify the reds**

```bash
cargo test -p wordcartel lsp_client:: 2>&1 | tail -8
```
Expected: `first_check_uses_the_long_deadline` and `respawn_re_enters_the_warm_phase` FAIL
(the normal watchdog fires at `PUBLISH_TIMEOUT_MS`); the other two PASS (they describe current
behavior — they are regression guards).

- [ ] **Step 3: Implement**

In `ClientState<E>`:
1. Add the field (with the others, `pub(crate)`): `pub(crate) first_publish_seen: bool,` —
   initialize `first_publish_seen: false,` in `new`.
2. In `on_spawned`, first line of the body: `self.first_publish_seen = false;`.
3. In `on_change`, compute once at the top:
   ```rust
        // E10 §4: until this child's first publish, the watchdog runs at the engine's
        // warm-phase deadline (JVM boot + model load land in first-CHECK latency).
        let publish_timeout = if self.first_publish_seen { E::PUBLISH_TIMEOUT_MS }
            else { E::FIRST_CHECK_TIMEOUT_MS.unwrap_or(E::PUBLISH_TIMEOUT_MS) };
   ```
   and replace BOTH `deadline: now + E::PUBLISH_TIMEOUT_MS` occurrences (reopen + didChange
   arms) with `deadline: now + publish_timeout`.
4. In `on_publish`, immediately after the `(tagged, text, lsp_version)` attribution match
   succeeds (before the version-echo guard — an attributed publish proves warmth even when the
   echo mismatches): `self.first_publish_seen = true;`.

- [ ] **Step 4: Run to verify green**

```bash
cargo test -p wordcartel lsp_client:: harper_ls:: 2>&1 | tail -5
```
Expected: all PASS (harper's pinned suite included — `None` engines are behavior-identical).

- [ ] **Step 5: Full gate + commit**

```bash
cargo test --workspace 2>&1 | tail -3 && cargo clippy --workspace --all-targets 2>&1 | tail -2
git add wordcartel/src/lsp_client.rs
git commit -m "feat: warm-phase first-check watchdog deadline in the LSP core (E10 T2)"
```

---

### Task 3: Suspend/resume machinery (core + provider seam)

**Files:**
- Modify: `wordcartel/src/lsp_client.rs`, `wordcartel/src/diag_provider.rs`
- Test: `lsp_client.rs` tests module (extend), `diag_provider.rs` tests module (extend)

**Interfaces:**
- Consumes: T1's core; T2's `TestEngine` (`SUSPENDABLE = true`).
- Produces: `Cmd::Suspend`, `Phase::Suspended`, `Action::{Park, Unpark}`;
  `DiagnosticsProvider::suspend(&mut self)` (defaulted no-op);
  `ProviderSet::suspend_all_idle_heavy(&mut self)`; `ProviderCall::Suspend`. T8 fires
  `suspend_all_idle_heavy`.

- [ ] **Step 1: Write the failing state-machine tests** (append to `lsp_client.rs::tests`)

```rust
    // ── T3: suspend/resume ──────────────────────────────────────────────────────────────────

    #[test]
    fn suspend_in_running_flushes_sends_shutdown_exit_then_parks() {
        let mut st = running();
        change(&mut st, 0, 5, 0); // an outstanding latch to flush
        let out = st.on_inbound(Inbound::Cmd(Cmd::Suspend), 0);
        assert_eq!(diag_dones(&out), vec![(BufferId(0), 5)], "flush-first (spec §5)");
        let methods: Vec<&str> = sends(&out).iter()
            .map(|v| v["method"].as_str().unwrap_or("")).collect();
        assert_eq!(methods, ["shutdown", "exit"], "best-effort polite teardown");
        assert!(out.iter().any(|a| matches!(a, Action::Park)), "then park");
        // Flush precedes the sends (terminal-guarantee ordering).
        let flush_idx = out.iter().position(|a| matches!(a, Action::Emit(_))).unwrap();
        let send_idx = out.iter().position(|a| matches!(a, Action::Send(_))).unwrap();
        assert!(flush_idx < send_idx);
    }

    #[test]
    fn suspend_shutdown_response_is_ignored_no_pending_registered() {
        let mut st = running();
        let out = st.on_inbound(Inbound::Cmd(Cmd::Suspend), 0);
        let shutdown_id = sends(&out)[0]["id"].as_u64().expect("shutdown id");
        // The fire-and-forget response routes to the unknown-id arm → nothing.
        let late = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":shutdown_id,
            "result":Value::Null})), 0);
        assert!(late.is_empty(), "no PendingKind was registered for the suspend shutdown");
    }

    #[test]
    fn server_eof_while_suspended_is_drained() {
        let mut st = running();
        st.on_inbound(Inbound::Cmd(Cmd::Suspend), 0);
        let out = st.on_inbound(Inbound::ServerEof, 0);
        assert!(out.is_empty(),
            "a deliberate kill's EOF: no flush, no respawn, no Restarted, no budget use");
        assert_eq!(st.spawn_attempts, 1, "budget untouched by the expected EOF");
    }

    #[test]
    fn shutdown_while_suspended_exits_directly_no_send() {
        let mut st = running();
        st.on_inbound(Inbound::Cmd(Cmd::Suspend), 0);
        st.on_inbound(Inbound::ServerEof, 0); // the expected EOF, drained
        let out = st.on_inbound(Inbound::Cmd(Cmd::Shutdown), 0);
        assert!(sends(&out).is_empty(), "no child — nothing to Send (spec §5, C8)");
        assert!(out.iter().any(|a| matches!(a, Action::Exit)));
    }

    #[test]
    fn suspend_outside_running_is_dropped_not_queued() {
        let mut st = ClientState::<TestEngine>::new(cfg());
        let spawn = st.on_spawned(0); // Initializing
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        assert!(st.on_inbound(Inbound::Cmd(Cmd::Suspend), 0).is_empty(),
            "suspend while Initializing: dropped, not queued");
        // Complete the handshake: the queue replay must NOT contain a Park.
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":{"capabilities":{}}})), 0);
        assert!(!out.iter().any(|a| matches!(a, Action::Park)),
            "a stale suspend must never replay against the fresh child");
    }

    #[test]
    fn change_while_suspended_queues_and_unparks_then_replays() {
        let mut st = running();
        st.on_inbound(Inbound::Cmd(Cmd::Suspend), 0);
        st.on_inbound(Inbound::ServerEof, 0);
        let out = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 9,
            path: None, text: "back".into() }), 0);
        assert!(out.iter().any(|a| matches!(a, Action::Unpark)), "a Change warrants a child");
        assert!(sends(&out).is_empty(), "nothing sent while parked");
        // Resume = the respawn path verbatim: on_spawned → handshake → queue replay.
        let spawn = st.on_spawned(0);
        assert_eq!(st.spawn_attempts, 1, "deliberate resume never consumes the budget (spec §5)");
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        let replay = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":{"capabilities":{}}})), 0);
        assert!(sends(&replay).iter().any(|v| v["method"] == "textDocument/didOpen"),
            "the queued change replays as a didOpen after the resume handshake");
    }
```

And the provider-seam tests (append to `diag_provider.rs::tests`):

```rust
    #[test]
    fn recording_provider_records_suspend_and_set_delegates() {
        let rec = RecordingProvider::new().with_source(DiagSource::LTeX);
        let calls = rec.calls_handle();
        let mut set = ProviderSet::default();
        set.install(Box::new(rec), true);
        set.suspend_all_idle_heavy();
        assert!(calls.lock().unwrap().iter().any(|c| matches!(c, ProviderCall::Suspend)),
            "suspend_all_idle_heavy reaches every entry; the recorder observes it");
    }
```

- [ ] **Step 2: Run to verify the reds**

```bash
cargo test -p wordcartel lsp_client:: diag_provider:: 2>&1 | tail -8
```
Expected: FAIL to COMPILE (`Cmd::Suspend`, `Action::Park`, `ProviderCall::Suspend` not defined) —
a compile-fail red is the correct first red for enum-variant work.

- [ ] **Step 3: Implement the state machine**

In `lsp_client.rs`:

1. `Cmd` gains `Suspend`; `Phase` gains `Suspended`; `Action` gains `Park` and `Unpark`.
2. Replace `on_inbound`'s body with (complete):
   ```rust
    pub(crate) fn on_inbound(&mut self, inb: Inbound, now: u64) -> Vec<Action> {
        match inb {
            Inbound::Cmd(c) => {
                // E10 §5: a suspend outside Running is DROPPED, never queued — a stale
                // suspend replayed after a resume handshake would re-kill the fresh child.
                if matches!(c, Cmd::Suspend) && self.phase != Phase::Running {
                    return Vec::new();
                }
                // Parked: a Change is the one cmd that warrants a new child — queue it and
                // wake the pump; everything else follows the ordinary pre-Running queueing.
                if self.phase == Phase::Suspended && matches!(c, Cmd::Change { .. }) {
                    self.queued.push(c);
                    return vec![Action::Unpark];
                }
                if self.phase != Phase::Running && !matches!(c, Cmd::Shutdown) {
                    // Pre-handshake: queue for replay. Configure only updates cfg (the
                    // handshake's didChangeConfiguration carries it) so it never double-applies.
                    match c {
                        Cmd::Configure(cfg) => self.cfg = cfg,
                        other => self.queued.push(other),
                    }
                    Vec::new()
                } else {
                    self.apply_cmd(c, now)
                }
            }
            Inbound::Server(v) => self.on_server(v, now),
            // E10 §5 (C7): a deliberate suspend kills the child; the reader's EOF is
            // EXPECTED — drained, never routed to the crash/respawn path.
            Inbound::ServerEof => if self.phase == Phase::Suspended { Vec::new() }
                else { self.on_server_gone(now) },
        }
    }
   ```
3. In `apply_cmd`, add the Suspend arm and make Shutdown phase-aware (complete arms):
   ```rust
            Cmd::Suspend => {
                // Running only (on_inbound drops it otherwise). Flush-first makes the
                // terminal guarantee unconditional; then best-effort polite teardown —
                // fire-and-forget: NO PendingKind, a late response hits the unknown-id arm.
                let mut out = self.flush_outstanding();
                let id = self.alloc_id();
                out.push(Action::Send(json!({"jsonrpc":"2.0","id":id,"method":"shutdown"})));
                out.push(Action::Send(json!({"jsonrpc":"2.0","method":"exit"})));
                out.push(Action::Park);
                self.phase = Phase::Suspended;
                out
            }
            Cmd::Shutdown => {
                // E10 §5 (C8): parked ⇒ no child — nothing to hand a shutdown request to;
                // outstanding work was flushed at suspend time. Straight to Exit.
                if self.phase == Phase::Suspended { return vec![Action::Exit]; }
                self.phase = Phase::ShuttingDown;
                let id = self.alloc_id();
                self.pending_requests.insert(id, PendingKind::Shutdown);
                vec![Action::Send(json!({"jsonrpc":"2.0","id":id,"method":"shutdown"}))]
            }
   ```
   (If `apply_cmd`'s match style makes the early `return` awkward, restructure that one arm as
   an `if/else` block expression — behavior identical.)
4. Restructure the pump for an optional child. `Control` gains `Park` and `Unpark`;
   `run_actions` signature changes to take the optional session:
   ```rust
    fn run_actions(acts: Vec<Action>, session: &mut Option<(Child, ChildStdin)>,
        msg_tx: &Sender<Msg>, shared: &Arc<Shared>) -> Control {
        for a in acts {
            match a {
                Action::Send(v) => match session {
                    Some((_, stdin)) => { let _ = write_frame_to(stdin, &v); }
                    None => debug_assert!(false, "Action::Send while parked (spec §5 rules make this unreachable)"),
                },
                Action::Emit(m) => { let _ = msg_tx.send(m); }
                Action::SetAvailability(av) => set_availability(shared, av),
                Action::Respawn => return Control::Respawn,
                Action::Exit => return Control::Exit,
                Action::Park => return Control::Park,
                Action::Unpark => return Control::Unpark,
            }
        }
        Control::Continue
    }
   ```
   In `pump<E>`: hold `let mut session: Option<(Child, ChildStdin)> = Some((child, stdin));`
   after the initial spawn (keep the existing spawn-failure early-return). Update every
   `&mut stdin` use to route through `session`. Extend the control match:
   ```rust
            match run_actions(acts, &mut session, &guard.msg_tx, shared) {
                Control::Continue => {}
                Control::Exit => break,
                Control::Respawn => { /* existing arm, but kill via session.take() and
                    reassign session = Some((c, s)) on success */ }
                Control::Park => {
                    // E10 §5: kill the child, keep the thread — a blocked thread is free;
                    // the JVM was the cost. Availability Idle = "at rest, will lazy-resume".
                    if let Some((mut c, s)) = session.take() { drop(s); let _ = c.kill(); let _ = c.wait(); }
                    set_availability(shared, Availability::Idle);
                }
                Control::Unpark => {
                    match spawn_session::<E>(inbound_tx) {
                        Ok(cs) => {
                            session = Some(cs);
                            let acts = guard.state.on_spawned(now(&start));
                            let _ = run_actions(acts, &mut session, &guard.msg_tx, shared);
                        }
                        Err(_) => {
                            // Resume-spawn failure = the spawn-failure path: degrade + flush
                            // the queued change so its accepted latch cannot wedge (spec §5).
                            set_availability(shared, Availability::Unavailable);
                            let _ = guard.msg_tx.send(Msg::DiagProviderEvent { source: E::SOURCE,
                                event: ProviderEvent::Degraded(E::INSTALL_HINT.into()) });
                            for a in guard.state.flush_outstanding() {
                                if let Action::Emit(m) = a { let _ = guard.msg_tx.send(m); }
                            }
                        }
                    }
                }
            }
   ```
   The pump's tail becomes `if let Some((mut c, _s)) = session.take() { let _ = c.kill(); let _ = c.wait(); }`.
5. Provider seam, in `diag_provider.rs`:
   - Trait: add after `reload_dictionary`:
     ```rust
    /// E10 §5: ask a heavy engine to release its child process until next summoned.
    /// Default no-op — only suspendable LSP providers (ltex) override behavior.
    fn suspend(&mut self) {}
     ```
   - `ProviderSet`:
     ```rust
    /// Fire the idle suspend at every entry — only SUSPENDABLE providers act (E10 §6).
    pub fn suspend_all_idle_heavy(&mut self) {
        for e in self.entries.iter_mut() { e.provider.suspend(); }
    }
     ```
   - `ProviderCall` gains `Suspend`; `RecordingProvider` gets
     `fn suspend(&mut self) { self.push(ProviderCall::Suspend); }` in its
     `DiagnosticsProvider` impl.
   - In `lsp_client.rs`, the `DiagnosticsProvider for LspProvider<E>` impl adds:
     ```rust
    fn suspend(&mut self) {
        if E::SUSPENDABLE && self.started { let _ = self.cmd_tx.send(Inbound::Cmd(Cmd::Suspend)); }
    }
     ```

- [ ] **Step 4: Run to verify green**

```bash
cargo test -p wordcartel lsp_client:: diag_provider:: harper_ls:: 2>&1 | tail -5
```
Expected: all PASS (harper's pinned suite included — `Suspended` is unreachable for a
never-suspended engine, and the EOF arm's else-branch is the old body).

- [ ] **Step 5: Full gate + commit**

```bash
cargo test --workspace 2>&1 | tail -3 && cargo clippy --workspace --all-targets 2>&1 | tail -2
git add wordcartel/src/lsp_client.rs wordcartel/src/diag_provider.rs
git commit -m "feat: suspend/resume (park-the-child) machinery in the LSP core (E10 T3)"
```

---

### Task 4: Config keys + `ProviderConfig.language` (the pin's sanctioned edit)

**Files:**
- Modify: `wordcartel/src/config.rs`, `wordcartel/src/diag_provider.rs`,
  `wordcartel/src/harper_ls.rs` (**tests only — the ONE sanctioned mechanical edit**),
  `wordcartel/src/diagnostics_run.rs` (the production `ProviderConfig` literal)
- Test: `config.rs` inline tests (extend)

**Interfaces:**
- Produces: `DiagnosticsConfig.{default_engine: Option<DiagSource>, ltex_language: String,
  ltex_idle_shutdown_min: u64}`; `ProviderConfig.language: Option<String>`. T5/T6/T8/T10 consume.

**Pin boundary note (spec §9):** the harper-tests-unmodified pin ends HERE, legitimately: adding
`language` breaks every `ProviderConfig` struct literal. The complete crate-wide literal census
(verified) is five sites — `harper_ls.rs::tests` `cfg()` helper and the
`settings_object_omits_dict_when_none_and_toggles_grammar` literal; `diag_provider.rs::tests`
`recording_provider_records_every_call_in_order` (two literals — the `configure` call and its
assertion); `diagnostics_run.rs::install_core_providers` (production). Each gets an explicit
`language: None,` (production harper) — no `Default` derive, no `..Default::default()` (house
explicitness; the exhaustive literal keeps the compiler forcing future field placement).

- [ ] **Step 1: Write the failing config tests** (append to `config.rs`'s test module)

```rust
    #[test]
    fn diagnostics_default_engine_folds_known_and_warns_unknown() {
        let (cfg, warns) = load_cfg("de_known.toml", r#"
[diagnostics]
default_engine = "ltex"
"#);
        assert_eq!(cfg.diagnostics.default_engine,
            Some(wordcartel_core::diagnostics::DiagSource::LTeX));
        assert!(warns.is_empty());

        let (cfg2, warns2) = load_cfg("de_unknown.toml", r#"
[diagnostics]
default_engine = "grammarly"
"#);
        assert_eq!(cfg2.diagnostics.default_engine, None, "unknown name → not set");
        assert!(warns2.iter().any(|w| w.contains("default_engine") && w.contains("grammarly")),
            "unknown default_engine warns with the known set");
    }

    #[test]
    fn diagnostics_ltex_table_folds_language_and_idle_with_defaults() {
        let (cfg, warns) = load_cfg("ltex_keys.toml", r#"
[diagnostics.ltex]
language = "de-DE"
idle_shutdown_min = 0
"#);
        assert!(warns.is_empty());
        assert_eq!(cfg.diagnostics.ltex_language, "de-DE");
        assert_eq!(cfg.diagnostics.ltex_idle_shutdown_min, 0, "0 = never suspend");

        let (dflt, _) = load_cfg("ltex_defaults.toml", "");
        assert_eq!(dflt.diagnostics.ltex_language, "en-US", "spec §9 default");
        assert_eq!(dflt.diagnostics.ltex_idle_shutdown_min, 15, "spec ruling 3 default");
        assert_eq!(dflt.diagnostics.default_engine, None);
    }
```

(`load_cfg(name, body) -> (Config, Vec<String>)` is the file's EXISTING test helper — reuse it
verbatim; do not invent a second harness. If its signature differs from these calls, match the
sibling `debounce_ms`-floor test's usage.)

- [ ] **Step 2: Run to verify the reds**

```bash
cargo test -p wordcartel config:: 2>&1 | tail -6
```
Expected: FAIL to COMPILE (`default_engine`/`ltex_language` fields missing).

- [ ] **Step 3: Implement the config surface**

In `config.rs`:

1. `RawDiagnostics` gains (keeping `harper: RawHarperEngine` as-is):
   ```rust
    default_engine: Option<String>,
    ltex: RawLtexEngine,
   ```
   and below `RawHarperEngine`:
   ```rust
/// `[diagnostics.ltex]` — the per-engine table for the ltex linter (E10 §9): the
/// LanguageTool language code and the JVM idle-suspend timeout (minutes; 0 = never).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawLtexEngine {
    language: Option<String>,
    idle_shutdown_min: Option<u64>,
}
   ```
2. `DiagnosticsConfig` (the cooked struct) gains:
   ```rust
    /// `[diagnostics] default_engine` — the lens the Review view seeds to at startup when the
    /// named engine is installed+enabled (E10 §13). Config-only: no command surface.
    pub default_engine: Option<wordcartel_core::diagnostics::DiagSource>,
    /// `[diagnostics.ltex] language` — the LanguageTool language code sent to ltex-ls-plus.
    pub ltex_language: String,
    /// `[diagnostics.ltex] idle_shutdown_min` — minutes of no-Review before the ltex JVM is
    /// suspended (E10 §6). `0` = keep warm forever.
    pub ltex_idle_shutdown_min: u64,
   ```
   with `Default` extended: `default_engine: None, ltex_language: "en-US".into(),
   ltex_idle_shutdown_min: 15,`.
3. The fold (beside the existing `raw.diagnostics.*` folds):
   ```rust
        if let Some(name) = raw.diagnostics.default_engine {
            use wordcartel_core::diagnostics::DiagSource;
            match name.as_str() {
                "harper" => cfg.diagnostics.default_engine = Some(DiagSource::Harper),
                "ltex" => cfg.diagnostics.default_engine = Some(DiagSource::LTeX),
                "vale" => cfg.diagnostics.default_engine = Some(DiagSource::Vale),
                other => warns.push(format!(
                    "config: diagnostics.default_engine — unknown engine \"{other}\" (known: harper, ltex, vale)")),
            }
        }
        if let Some(v) = raw.diagnostics.ltex.language { cfg.diagnostics.ltex_language = v; }
        if let Some(v) = raw.diagnostics.ltex.idle_shutdown_min {
            cfg.diagnostics.ltex_idle_shutdown_min = v;
        }
   ```

In `diag_provider.rs`, `ProviderConfig` gains:
```rust
    /// Engine language code (ltex); `None` for engines without a language knob (E10 §9 —
    /// the one engine-varying field; harper/vale receive `None` and ignore it).
    pub language: Option<String>,
```
and the `#[cfg(test)] impl PartialEq for ProviderConfig` adds `&& self.language == other.language`.

Then the five literal sites, mechanically:
- `harper_ls.rs::tests::cfg()` → `ProviderConfig { grammar, dictionary: None, max_file_length: 10_000, language: None }`
- `harper_ls.rs::tests::settings_object_omits_dict_when_none_and_toggles_grammar` →
  add `language: None` to its `ProviderConfig { grammar: true, dictionary: Some("/d.txt".into()), max_file_length: 5, … }` literal
- `diag_provider.rs::tests` — both `ProviderConfig { grammar: false, dictionary: Some("/d".into()), max_file_length: 9 }` literals gain `, language: None`
- `diagnostics_run.rs::install_core_providers`'s harper construction gains `language: None,`
- ALSO: `lsp_client.rs::tests::cfg()` (added in T2) gains `language: None`.

- [ ] **Step 4: Run to verify green + the pin diff**

```bash
cargo test --workspace 2>&1 | tail -3 && cargo clippy --workspace --all-targets 2>&1 | tail -2
git diff wordcartel/src/harper_ls.rs
```
Expected: green/clean; the harper diff shows EXACTLY two hunks, each adding `language: None` —
nothing else (the reviewer checks this).

- [ ] **Step 5: Commit**

```bash
git add wordcartel/src/config.rs wordcartel/src/diag_provider.rs wordcartel/src/harper_ls.rs \
    wordcartel/src/diagnostics_run.rs wordcartel/src/lsp_client.rs
git commit -m "feat: per-engine config keys + ProviderConfig.language (E10 T4)"
```

---

### Task 5: The ltex engine (`ltex_ls.rs`) + catalog arm

**Files:**
- Create: `wordcartel/src/ltex_ls.rs`
- Modify: `wordcartel/src/lib.rs` (mod line), `wordcartel/src/lsp_client.rs`
  (`classify_spell_heuristic`), `wordcartel/src/diagnostics_run.rs` (catalog + warning text)
- Test: inline in `ltex_ls.rs` + `diagnostics_run.rs` (extend `install_core_providers` tests)

**Interfaces:**
- Consumes: T1 core, T2 warm machinery, T4 `ProviderConfig.language` +
  `DiagnosticsConfig.ltex_language`.
- Produces: `ltex_ls::LtexEngine`; `lsp_client::classify_spell_heuristic`;
  the catalog `&[DiagSource::Harper, DiagSource::LTeX]` (T6 appends Vale). T7/T9 rely on the
  LTeX entry existing in `ProviderSet`.

- [ ] **Step 1: Write the failing tests**

`wordcartel/src/ltex_ls.rs` (create with the tests first — the impl in Step 3 completes it):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_client::{ClientState, Cmd, Inbound, LspEngine};
    use crate::diag_provider::ProviderConfig;
    use crate::editor::BufferId;
    use serde_json::{json, Value};

    fn cfg() -> ProviderConfig {
        ProviderConfig { grammar: true, dictionary: None, max_file_length: 10_000,
            language: Some("de-DE".into()) }
    }
    fn sends(acts: &[crate::lsp_client::Action]) -> Vec<&Value> {
        acts.iter().filter_map(|a| if let crate::lsp_client::Action::Send(v) = a { Some(v) } else { None }).collect()
    }

    #[test]
    fn constants_match_the_spec() {
        assert_eq!(LtexEngine::PUBLISH_TIMEOUT_MS, 15_000);
        assert_eq!(LtexEngine::FIRST_CHECK_TIMEOUT_MS, Some(180_000));
        assert!(LtexEngine::SUSPENDABLE);
        assert!(LtexEngine::INSTALL_HINT.contains("Java 21+"), "ruling 6 copy");
    }

    #[test]
    fn settings_push_is_nested_under_ltex_with_the_cfg_language() {
        let push = LtexEngine::settings_push(&cfg()).expect("ltex pushes");
        assert_eq!(push["ltex"]["language"], json!("de-DE"));
    }

    #[test]
    fn answer_request_serves_both_pull_methods_bare_per_item() {
        for method in ["workspace/configuration", "ltex/workspaceSpecificConfiguration"] {
            let req = json!({"jsonrpc":"2.0","id":7,"method":method,
                "params":{"items":[{},{}]}});
            let result = LtexEngine::answer_request(method, &req, &cfg()).expect("answered");
            let arr = result.as_array().expect("array per items");
            assert_eq!(arr.len(), 2);
            assert!(arr[0].get("ltex").is_none(), "BARE settings, not nested (harper MUST-FIX shape)");
            assert_eq!(arr[0]["language"], json!("de-DE"));
        }
        assert!(LtexEngine::answer_request("ltex/other", &json!({}), &cfg()).is_none());
    }

    #[test]
    fn classify_maps_languagetool_speller_rules_to_spelling() {
        use wordcartel_core::diagnostics::DiagnosticKind;
        assert_eq!(LtexEngine::classify(&json!({"code":"MORFOLOGIK_RULE_EN_US","message":"x"})),
            DiagnosticKind::Spelling);
        assert_eq!(LtexEngine::classify(&json!({"code":"GERMAN_SPELLER_RULE","message":"x"})),
            DiagnosticKind::Spelling);
        assert_eq!(LtexEngine::classify(&json!({"code":"PASSIVE_VOICE","message":"x"})),
            DiagnosticKind::Grammar);
        assert_eq!(LtexEngine::classify(&json!({"message":"Possible spelling mistake"})),
            DiagnosticKind::Spelling, "falls through to the shared heuristic");
    }

    #[test]
    fn first_ltex_check_rides_the_180s_warm_deadline() {
        let mut st = ClientState::<LtexEngine>::new(cfg());
        let spawn = st.on_spawned(0);
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":{"capabilities":{}}})), 0);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1,
            path: None, text: "x".into() }), 0);
        assert!(st.on_deadline(15_000).is_empty(), "no false-empty at the steady watchdog");
        assert!(!st.on_deadline(180_000).is_empty(), "the warm deadline eventually terminates");
    }
}
```

And in `diagnostics_run.rs`'s tests, extend the catalog expectations:

```rust
    #[test]
    fn install_core_providers_registers_ltex_after_harper() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut warns = Vec::new();
        install_core_providers(&mut e, &crate::config::Config::default(), &tx, &mut warns);
        let sources: Vec<DiagSource> = e.diag_providers.sources().collect();
        assert!(sources.starts_with(&[DiagSource::Harper, DiagSource::LTeX]),
            "cycle order: harper first, ltex second (spec §13 catalog)");
        assert!(warns.is_empty());
    }
```

(Mirror the arrange style of the EXISTING `install_core_providers_*` tests in that file — reuse
their helper if one exists.)

- [ ] **Step 2: Run to verify the reds**

```bash
cargo test -p wordcartel ltex_ls:: diagnostics_run::tests::install_core_providers_registers 2>&1 | tail -5
```
Expected: FAIL to COMPILE (`LtexEngine` undefined; module missing).

- [ ] **Step 3: Implement**

`wordcartel/src/lib.rs`: add `mod ltex_ls;`.

`lsp_client.rs`: add (near `classify`-related code; production, not cfg(test) — ltex/vale use it):

```rust
/// The shared spelling-vs-grammar fallback heuristic (E10 §7/§8): a lowercase "spell"
/// substring across code/source/message. Engine classifiers try their own rule-id tables
/// first and fall through to this. (harper's `classify_lsp` keeps its original private copy —
/// the T1 pin; the two bodies are intentionally identical.)
pub(crate) fn classify_spell_heuristic(d: &Value) -> DiagnosticKind {
    let code = match d.get("code") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    if code.to_lowercase().contains("spell") { return DiagnosticKind::Spelling; }
    let source = d.get("source").and_then(|s| s.as_str()).unwrap_or("");
    let message = d.get("message").and_then(|m| m.as_str()).unwrap_or("");
    if format!("{source} {message}").to_lowercase().contains("spell") {
        DiagnosticKind::Spelling
    } else {
        DiagnosticKind::Grammar
    }
}
```

`ltex_ls.rs` production half (above the tests):

```rust
//! The ltex-ls-plus engine (E10 §7): LanguageTool grammar/language diagnostics over the
//! engine-generic `lsp_client` core. The JVM outlier — warm-phase watchdog (180 s first
//! check) + idle suspend (the only SUSPENDABLE engine). Empirical protocol details
//! (spawn flags, the custom config request's response shape) are validated by the T11
//! live probe against the real binary.
use serde_json::{json, Value};

use crate::diag_provider::ProviderConfig;
use crate::lsp_client::LspEngine;
use wordcartel_core::diagnostics::{DiagnosticKind, DiagSource};

/// Status hint when ltex-ls-plus is unavailable (E10 ruling 6).
pub const INSTALL_HINT: &str =
    "language checker unavailable — install ltex-ls-plus (requires Java 21+)";
/// Degrade hint once the respawn budget is exhausted.
const CRASHED_HINT: &str = "language checker stopped after repeated restarts";

/// The ltex-ls-plus engine spec (E10 §7).
#[derive(Debug)]
pub(crate) struct LtexEngine;

impl LspEngine for LtexEngine {
    const SOURCE: DiagSource = DiagSource::LTeX;
    const INSTALL_HINT: &'static str = INSTALL_HINT;
    const CRASHED_HINT: &'static str = CRASHED_HINT;
    const LANGUAGE_ID: &'static str = "markdown";
    const CLIENT_THREAD: &'static str = "wcartel-ltex-client";
    const READER_THREAD: &'static str = "wcartel-ltex-read";
    /// LanguageTool re-checks are slower than harper's — 15 s steady-state (spec §4).
    const PUBLISH_TIMEOUT_MS: u64 = 15_000;
    /// The JVM + model warm lands in first-CHECK latency: 2-min worst case + margin (spec §4).
    const FIRST_CHECK_TIMEOUT_MS: Option<u64> = Some(180_000);
    const CODEACTION_TIMEOUT_MS: u64 = 5_000;
    const SUSPENDABLE: bool = true;

    // T11-probe flag: the bare invocation is the documented stdio default; verify live.
    fn spawn_command() -> std::process::Command {
        std::process::Command::new("ltex-ls-plus")
    }

    fn initialize_params(_cfg: &ProviderConfig) -> Value {
        json!({
            "processId": std::process::id(),
            "rootUri": Value::Null,
            "clientInfo": {"name":"wordcartel","version": env!("CARGO_PKG_VERSION")},
            "initializationOptions": Value::Null,
            "capabilities": {
                "workspace": {"configuration": true,
                    "didChangeConfiguration": {"dynamicRegistration": false}},
                "textDocument": {
                    "publishDiagnostics": {"versionSupport": true},
                    "codeAction": {"dynamicRegistration": false}
                }
            }
        })
    }

    /// Nested under `"ltex"` — the push-as-re-pull-trigger (harper's pattern, ltex's section).
    fn settings_push(cfg: &ProviderConfig) -> Option<Value> {
        Some(json!({"ltex": ltex_settings(cfg)}))
    }

    /// Both the standard PULL and ltex-plus's custom merge extension get the same bare,
    /// per-item settings objects (harper's MUST-FIX shape; T11-probe flag on the custom
    /// method's expected response schema).
    fn answer_request(method: &str, req: &Value, cfg: &ProviderConfig) -> Option<Value> {
        match method {
            "workspace/configuration" | "ltex/workspaceSpecificConfiguration" => {
                let items = req["params"]["items"].as_array().map(|a| a.len()).unwrap_or(1);
                let obj = ltex_settings(cfg);
                Some(Value::Array((0..items).map(|_| obj.clone()).collect()))
            }
            _ => None,
        }
    }

    /// LanguageTool speller rule ids → Spelling; everything else falls to the shared
    /// heuristic (spec §7: MORFOLOGIK_RULE_* / *_SPELLER_RULE / HUNSPELL_*).
    fn classify(d: &Value) -> DiagnosticKind {
        if let Some(code) = d.get("code").and_then(|c| c.as_str()) {
            let up = code.to_uppercase();
            if up.contains("MORFOLOGIK") || up.contains("HUNSPELL") || up.contains("SPELLER") {
                return DiagnosticKind::Spelling;
            }
        }
        crate::lsp_client::classify_spell_heuristic(d)
    }
}

/// The bare ltex settings object — `language` is the sole E10 key (spec §9).
fn ltex_settings(cfg: &ProviderConfig) -> Value {
    json!({"language": cfg.language.as_deref().unwrap_or("en-US")})
}
```

`diagnostics_run.rs`, in `install_core_providers`:
- catalog: `let catalog: &[DiagSource] = &[DiagSource::Harper, DiagSource::LTeX];`
  (T6 appends Vale — update the neighboring "Effort b appends" comment to say Vale lands in T6).
- the construction match gains:
  ```rust
            DiagSource::LTeX => Box::new(crate::lsp_client::LspProvider::<crate::ltex_ls::LtexEngine>::new(
                msg_tx.clone(),
                crate::diag_provider::ProviderConfig {
                    grammar: cfg.diagnostics.grammar,
                    dictionary: None, // per-engine dictionaries are E11's (spec §14.2)
                    max_file_length: crate::limits::HARPER_MAX_FILE_LENGTH, // inert for ltex (spec §9)
                    language: Some(cfg.diagnostics.ltex_language.clone()),
                })),
  ```
  (keep the exhaustive-match arm for the not-yet-in-catalog `DiagSource::Vale | DiagSource::Plugin(_) => continue`).
- the unknown-linters warning derives its known set from the catalog instead of the hardcoded
  `"(known: harper)"`:
  ```rust
                warns.push(format!(
                    "config: diagnostics.linters — unknown engine \"{name}\" (known: {})",
                    catalog.iter().map(|s| s.config_name()).collect::<Vec<_>>().join(", ")));
  ```
  If an existing test asserts the old `(known: harper)` text verbatim, update THAT assertion to
  the derived form — it is not part of the harper pin.

- [ ] **Step 4: Run to verify green, full gate, commit**

```bash
cargo test --workspace 2>&1 | tail -3 && cargo clippy --workspace --all-targets 2>&1 | tail -2
git add wordcartel/src/ltex_ls.rs wordcartel/src/lsp_client.rs wordcartel/src/lib.rs \
    wordcartel/src/diagnostics_run.rs
git commit -m "feat: ltex-ls-plus engine spec + catalog arm (E10 T5)"
```

---

### Task 6: The vale engine (`vale_ls.rs`) + catalog completion

**Files:**
- Create: `wordcartel/src/vale_ls.rs`
- Modify: `wordcartel/src/lib.rs` (mod line), `wordcartel/src/diagnostics_run.rs` (catalog)
- Test: inline in `vale_ls.rs` + extend `diagnostics_run.rs`

**Interfaces:**
- Consumes: T1 core, T4 config.
- Produces: `vale_ls::ValeEngine`; the complete catalog
  `&[DiagSource::Harper, DiagSource::LTeX, DiagSource::Vale]`.

- [ ] **Step 1: Write the failing tests** (`vale_ls.rs`, tests-first)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_client::LspEngine;
    use crate::diag_provider::ProviderConfig;
    use serde_json::json;

    fn cfg() -> ProviderConfig {
        ProviderConfig { grammar: true, dictionary: None, max_file_length: 10_000, language: None }
    }

    #[test]
    fn constants_match_the_spec() {
        assert_eq!(ValeEngine::PUBLISH_TIMEOUT_MS, 10_000);
        assert_eq!(ValeEngine::FIRST_CHECK_TIMEOUT_MS, None, "no warm phase (Go chain)");
        assert!(!ValeEngine::SUSPENDABLE, "vale is never suspended (ruling 3)");
    }

    #[test]
    fn initialize_pins_install_vale_false_and_no_config_pull() {
        let params = ValeEngine::initialize_params(&cfg());
        assert_eq!(params["initializationOptions"]["installVale"], json!(false),
            "ruling 4: NO auto-install, ever");
        assert_eq!(params["initializationOptions"]["syncOnStartup"], json!(false));
        assert_eq!(params["capabilities"]["workspace"]["configuration"], json!(false),
            "vale-ls takes no config exchange — do not invite a PULL");
    }

    #[test]
    fn vale_never_pushes_settings_and_answers_no_requests() {
        assert!(ValeEngine::settings_push(&cfg()).is_none());
        assert!(ValeEngine::answer_request("workspace/configuration", &json!({}), &cfg()).is_none());
    }

    #[test]
    fn classify_spelling_checks_by_name_else_heuristic() {
        use wordcartel_core::diagnostics::DiagnosticKind;
        assert_eq!(ValeEngine::classify(&json!({"code":"Vale.Spelling","message":"x"})),
            DiagnosticKind::Spelling);
        assert_eq!(ValeEngine::classify(&json!({"code":"write-good.Passive","message":"x"})),
            DiagnosticKind::Grammar);
    }
}
```

Extend `diagnostics_run.rs` — modify the T5 catalog test (same test, full expectation):

```rust
        assert_eq!(sources, vec![DiagSource::Harper, DiagSource::LTeX, DiagSource::Vale],
            "the complete E10 catalog in cycle order");
```

- [ ] **Step 2: Run to verify the reds** — `cargo test -p wordcartel vale_ls:: 2>&1 | tail -4`
  → compile FAIL (module missing); the catalog test FAILS (2 sources).

- [ ] **Step 3: Implement**

`lib.rs`: `mod vale_ls;`. Then `vale_ls.rs` production half:

```rust
//! The vale-ls engine (E10 §8): prose style diagnostics via Vale's LSP wrapper, over the
//! engine-generic `lsp_client` core. The near-free third arm: no warm phase, never
//! suspended, no config exchange (vale reads `.vale.ini` itself via its own discovery —
//! cwd-relative under our opaque URIs, a stated E10 limitation). `installVale` is pinned
//! FALSE: this app never downloads binaries (ruling 4). T11 probes the real binary.
use serde_json::{json, Value};

use crate::diag_provider::ProviderConfig;
use crate::lsp_client::LspEngine;
use wordcartel_core::diagnostics::{DiagnosticKind, DiagSource};

/// Status hint when vale/vale-ls are unavailable (E10 §8).
pub const INSTALL_HINT: &str = "style linter unavailable — install vale and vale-ls";
/// Degrade hint once the respawn budget is exhausted.
const CRASHED_HINT: &str = "style linter stopped after repeated restarts";

/// The vale-ls engine spec (E10 §8).
#[derive(Debug)]
pub(crate) struct ValeEngine;

impl LspEngine for ValeEngine {
    const SOURCE: DiagSource = DiagSource::Vale;
    const INSTALL_HINT: &'static str = INSTALL_HINT;
    const CRASHED_HINT: &'static str = CRASHED_HINT;
    const LANGUAGE_ID: &'static str = "markdown";
    const CLIENT_THREAD: &'static str = "wcartel-vale-client";
    const READER_THREAD: &'static str = "wcartel-vale-read";
    const PUBLISH_TIMEOUT_MS: u64 = 10_000;
    const FIRST_CHECK_TIMEOUT_MS: Option<u64> = None;
    const CODEACTION_TIMEOUT_MS: u64 = 5_000;
    const SUSPENDABLE: bool = false;

    // T11-probe flag: bare stdio invocation; verify live.
    fn spawn_command() -> std::process::Command {
        std::process::Command::new("vale-ls")
    }

    // T11-probe flag: initializationOptions key names (unrecognized keys are ignored —
    // LSP init options are freeform — but the probe confirms the pin takes effect).
    fn initialize_params(_cfg: &ProviderConfig) -> Value {
        json!({
            "processId": std::process::id(),
            "rootUri": Value::Null,
            "clientInfo": {"name":"wordcartel","version": env!("CARGO_PKG_VERSION")},
            "initializationOptions": {"installVale": false, "syncOnStartup": false},
            "capabilities": {
                "workspace": {"configuration": false,
                    "didChangeConfiguration": {"dynamicRegistration": false}},
                "textDocument": {
                    "publishDiagnostics": {"versionSupport": true},
                    "codeAction": {"dynamicRegistration": false}
                }
            }
        })
    }

    fn settings_push(_cfg: &ProviderConfig) -> Option<Value> { None }

    fn answer_request(_method: &str, _req: &Value, _cfg: &ProviderConfig) -> Option<Value> {
        None // generic handling only — vale-ls's hover/completion are config-file-only (scan trap)
    }

    /// Check names carrying "Spelling" → Spelling; else the shared heuristic (spec §8).
    fn classify(d: &Value) -> DiagnosticKind {
        if let Some(code) = d.get("code").and_then(|c| c.as_str()) {
            if code.contains("Spelling") { return DiagnosticKind::Spelling; }
        }
        crate::lsp_client::classify_spell_heuristic(d)
    }
}
```

`diagnostics_run.rs`: catalog becomes
`&[DiagSource::Harper, DiagSource::LTeX, DiagSource::Vale]`; the match gains:
```rust
            DiagSource::Vale => Box::new(crate::lsp_client::LspProvider::<crate::vale_ls::ValeEngine>::new(
                msg_tx.clone(),
                crate::diag_provider::ProviderConfig {
                    grammar: cfg.diagnostics.grammar,
                    dictionary: None,
                    max_file_length: crate::limits::HARPER_MAX_FILE_LENGTH, // inert for vale (spec §9)
                    language: None,
                })),
```
and the fallthrough arm narrows to `DiagSource::Plugin(_) => continue,` (exhaustive — the
compiler confirms every core engine now has an arm).

- [ ] **Step 4: Green, gate, commit**

```bash
cargo test --workspace 2>&1 | tail -3 && cargo clippy --workspace --all-targets 2>&1 | tail -2
git add wordcartel/src/vale_ls.rs wordcartel/src/lib.rs wordcartel/src/diagnostics_run.rs
git commit -m "feat: vale-ls engine spec, catalog complete at N=3 (E10 T6)"
```

---

### Task 7: The four command siblings

**Files:**
- Modify: `wordcartel/src/registry.rs`
- Test: `registry.rs` inline tests (extend the existing analysis-command block)

**Interfaces:**
- Consumes: `Editor::set_analysis_source(DiagSource)` (existing shared setter; refuses disabled
  engines), `diagnostics_run::set_engine_enabled(editor, source, on, clock)` (existing shared
  setter). T5/T6's ProviderSet entries.
- Produces: command ids `analysis_engine_ltex`, `analysis_engine_vale`, `toggle_engine_ltex`,
  `toggle_engine_vale` — T9's menu rows dispatch the toggles.

- [ ] **Step 1: Write the failing tests** — beside the existing
  `analysis_lens_commands_are_registered_with_correct_surface` block (mirror its style):

```rust
    #[test]
    fn ltex_vale_command_siblings_are_registered_palette_only() {
        let reg = Registry::builtins(); // the constructor the neighboring tests use
        for id in ["analysis_engine_ltex", "analysis_engine_vale",
                   "toggle_engine_ltex", "toggle_engine_vale"] {
            let meta = reg.meta(CommandId(id)).unwrap_or_else(|| panic!("{id} registered"));
            assert_eq!(meta.menu, None, "{id} is palette-only (contract rule 8 set primitives)");
        }
    }

    #[test]
    fn analysis_engine_ltex_dispatches_the_shared_setter() {
        let mut ed = editor_with_all_engines_enabled(); // see arrange note below
        dispatch_id(&mut ed, "analysis_engine_ltex");
        assert_eq!(ed.active_analysis_source, wordcartel_core::diagnostics::DiagSource::LTeX);
    }

    #[test]
    fn toggle_engine_vale_flips_enablement_via_set_engine_enabled() {
        let mut ed = editor_with_all_engines_enabled();
        dispatch_id(&mut ed, "toggle_engine_vale");
        assert!(!ed.diag_providers.is_enabled(wordcartel_core::diagnostics::DiagSource::Vale));
        dispatch_id(&mut ed, "toggle_engine_vale");
        assert!(ed.diag_providers.is_enabled(wordcartel_core::diagnostics::DiagSource::Vale));
    }
```

**Arrange note:** the existing sibling tests (`analysis_next_dispatches_cycle_and_reports_state`,
`toggle_engine_harper_dispatches_set_engine_enabled`) already build an editor with installed
recording providers and a `dispatch_id` helper — REUSE their exact arrange (copy the pattern into
a shared helper `editor_with_all_engines_enabled()` installing three
`RecordingProvider`s with sources Harper/LTeX/Vale, all enabled — or inline the same three lines
each test, matching the file's local style).

- [ ] **Step 2: Verify the reds** — `cargo test -p wordcartel registry:: 2>&1 | tail -5` →
  FAIL (`meta(...)` None for the new ids).

- [ ] **Step 3: Implement** — in `registry.rs`, directly under the existing
  `toggle_engine_harper` registration (the "the ltex/vale effort adds its siblings here" comment):

```rust
        r.register("analysis_engine_ltex", "Analysis Engine: LTeX", None, |c| {
            c.editor.set_analysis_source(wordcartel_core::diagnostics::DiagSource::LTeX);
            CommandResult::Handled
        });
        r.register("analysis_engine_vale", "Analysis Engine: vale", None, |c| {
            c.editor.set_analysis_source(wordcartel_core::diagnostics::DiagSource::Vale);
            CommandResult::Handled
        });
        r.register("toggle_engine_ltex", "Toggle LTeX Engine", None, |c| {
            let on = !c.editor.diag_providers.is_enabled(wordcartel_core::diagnostics::DiagSource::LTeX);
            crate::diagnostics_run::set_engine_enabled(c.editor,
                wordcartel_core::diagnostics::DiagSource::LTeX, on, c.clock);
            CommandResult::Handled
        });
        r.register("toggle_engine_vale", "Toggle vale Engine", None, |c| {
            let on = !c.editor.diag_providers.is_enabled(wordcartel_core::diagnostics::DiagSource::Vale);
            crate::diagnostics_run::set_engine_enabled(c.editor,
                wordcartel_core::diagnostics::DiagSource::Vale, on, c.clock);
            CommandResult::Handled
        });
```

`analysis_next` is NOT touched (it already cycles `enabled_sources()`). The palette-completeness
invariant derives rows from `reg.commands()` — the new ids join automatically; if an inventory
test enumerates expected command ids explicitly, add the four there.

- [ ] **Step 4: Green, gate, commit**

```bash
cargo test --workspace 2>&1 | tail -3 && cargo clippy --workspace --all-targets 2>&1 | tail -2
git add wordcartel/src/registry.rs
git commit -m "feat: ltex/vale analysis + toggle command siblings (E10 T7)"
```

---

### Task 8: Idle-shutdown app wiring (editor state + transition seam + timers row + fire)

**Files:**
- Modify: `wordcartel/src/editor.rs`, `wordcartel/src/diagnostics_run.rs`,
  `wordcartel/src/app.rs` (`reduce`), `wordcartel/src/timers.rs`
- Test: `diagnostics_run.rs` + `timers.rs` inline tests

**Interfaces:**
- Consumes: T3's `ProviderSet::suspend_all_idle_heavy` + `ProviderCall::Suspend`; T4's
  `ltex_idle_shutdown_min`; the existing `should_run_diagnostics`,
  `timers::{TimedSubsystem, SUBSYSTEMS, next_wake, on_tick}`.
- Produces: `Editor.diag_idle_due: Option<u64>`;
  `diagnostics_run::{idle_shutdown_track, diag_idle_fire}`.

- [ ] **Step 1: Write the failing tests** (append to `diagnostics_run.rs::tests`; use
  `crate::test_support::TestClock` and `RecordingProvider` with `DiagSource::LTeX` — the arrange
  style of `restarted_sets_status_and_arms_when_review_and_enabled` in `diag_provider.rs`):

```rust
    fn ltex_enabled_editor() -> crate::editor::Editor {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        e.diag_providers.install(Box::new(crate::diag_provider::RecordingProvider::new()
            .with_source(DiagSource::LTeX)), true);
        e.diag_cfg.enabled = true;
        e
    }

    #[test]
    fn leaving_review_arms_the_idle_due_and_reentry_clears_it() {
        use crate::test_support::TestClock;
        let mut e = ltex_enabled_editor();
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        // true → false (mode change out of Review).
        let before = should_run_diagnostics(&e);
        e.active_mut().view.mode = crate::editor::RenderMode::LivePreview;
        idle_shutdown_track(&mut e, before, &TestClock::new(1_000));
        assert_eq!(e.diag_idle_due, Some(1_000 + 15 * 60_000), "default 15 min (spec §6)");
        // false → true (re-entry) clears.
        let before = should_run_diagnostics(&e);
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        idle_shutdown_track(&mut e, before, &TestClock::new(2_000));
        assert_eq!(e.diag_idle_due, None, "the grace: re-entry cancels");
    }

    #[test]
    fn buffer_switch_out_of_review_also_arms() {
        use crate::test_support::TestClock;
        let mut e = ltex_enabled_editor();
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        e.install_scratch(); // a second, non-Review buffer
        let before = should_run_diagnostics(&e); // true (active is the Review buffer)
        e.switch_to_index(1); // scratch — Draft mode
        idle_shutdown_track(&mut e, before, &TestClock::new(500));
        assert!(e.diag_idle_due.is_some(),
            "the predicate transition fires on buffer switches, not only set_render_mode (spec §6)");
    }

    #[test]
    fn no_arm_when_ltex_disabled_or_zero_config_or_no_transition() {
        use crate::test_support::TestClock;
        // Disabled ltex: never arms.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        e.diag_cfg.enabled = true;
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        let before = should_run_diagnostics(&e);
        e.active_mut().view.mode = crate::editor::RenderMode::LivePreview;
        idle_shutdown_track(&mut e, before, &TestClock::new(0));
        assert_eq!(e.diag_idle_due, None, "no ltex entry → no arm");
        // Zero config: never arms.
        let mut e = ltex_enabled_editor();
        e.diag_cfg.ltex_idle_shutdown_min = 0;
        e.active_mut().view.mode = crate::editor::RenderMode::Review;
        let before = should_run_diagnostics(&e);
        e.active_mut().view.mode = crate::editor::RenderMode::LivePreview;
        idle_shutdown_track(&mut e, before, &TestClock::new(0));
        assert_eq!(e.diag_idle_due, None, "0 = keep warm forever (ruling 3)");
        // No transition: staying out of Review is a no-op (edge-triggered, not level).
        let mut e = ltex_enabled_editor();
        idle_shutdown_track(&mut e, false, &TestClock::new(0));
        assert_eq!(e.diag_idle_due, None);
    }

    #[test]
    fn diag_idle_fire_suspends_and_clears_once() {
        let mut e = ltex_enabled_editor();
        let rec = crate::diag_provider::RecordingProvider::new().with_source(DiagSource::Harper);
        let calls = rec.calls_handle();
        e.diag_providers.install(Box::new(rec), true);
        e.diag_idle_due = Some(1_000);
        diag_idle_fire(&mut e, 999);
        assert!(e.diag_idle_due.is_some(), "not yet due");
        diag_idle_fire(&mut e, 1_000);
        assert_eq!(e.diag_idle_due, None, "one-shot: cleared on fire");
        assert!(calls.lock().unwrap().iter().any(|c|
            matches!(c, crate::diag_provider::ProviderCall::Suspend)),
            "fire delegates to suspend_all_idle_heavy (every entry; SUSPENDABLE gating is provider-side)");
    }
```

And in `timers.rs::tests` (the idle-free guardrail, `pos_sweep` precedent — mirror the existing
`next_wake`-isolation tests' arrange):

```rust
    #[test]
    fn diag_idle_row_wakes_only_when_armed() {
        let mut e = Editor::new_from_text("x\n", None, (40, 10));
        assert_eq!(e.diag_idle_due, None);
        assert_eq!(next_wake(&e, 0), None, "unarmed ⇒ the row contributes nothing (idle-free)");
        e.diag_idle_due = Some(5_000);
        assert_eq!(next_wake(&e, 0), Some(5_000), "armed ⇒ the loop wakes at the due");
    }
```

(If a fresh `Editor` has other armed subsystems making `next_wake` non-None, follow the
neighboring guardrail tests' isolation pattern — they select the row by
`SUBSYSTEMS.iter().find(|s| s.name == "diag_idle")` and call its `deadline` fn directly; use
that form instead of whole-loop `next_wake`.)

- [ ] **Step 2: Verify the reds** — compile FAIL (`diag_idle_due`, `idle_shutdown_track`
  undefined).

- [ ] **Step 3: Implement**

1. `editor.rs` — beside `pub diag_hint_shown: …` (the E10 §6 home):
   ```rust
    /// E10 §6: the armed idle-suspend deadline for the heavy (ltex) engine — `Some(due_ms)`
    /// after a leaving-Review transition, cleared on re-entry or fire. Read by the
    /// `timers.rs` "diag_idle" row; never persisted.
    pub diag_idle_due: Option<u64>,
   ```
   and `diag_idle_due: None,` in the constructor beside `diag_hint_shown`'s init.
2. `diagnostics_run.rs`:
   ```rust
/// E10 §6: observe the summon-predicate TRANSITION at the reduce-exit seam (the
/// arm_if_edited chokepoint — every normal reduce exit; the sole bypass is the debug-only
/// WCARTEL_SMOKE_PANIC branch, a panic path). Arm the idle-suspend deadline on leaving
/// Review (mode change OR buffer switch), clear it on re-entry. Edge-triggered, never
/// level-triggered (the resource law). The arm gate is ENABLEMENT only — started-ness is
/// guarded provider-side (spec §6: no accessor; LspProvider::suspend no-ops unless
/// SUSPENDABLE && started).
pub fn idle_shutdown_track(editor: &mut Editor, summoned_before: bool,
    clock: &dyn wordcartel_core::history::Clock) {
    let summoned_now = should_run_diagnostics(editor);
    if summoned_before && !summoned_now {
        if editor.diag_providers.is_enabled(DiagSource::LTeX)
            && editor.diag_cfg.ltex_idle_shutdown_min > 0
        {
            editor.diag_idle_due = Some(clock.now_ms()
                .saturating_add(editor.diag_cfg.ltex_idle_shutdown_min.saturating_mul(60_000)));
        }
    } else if !summoned_before && summoned_now {
        editor.diag_idle_due = None;
    }
}

/// E10 §6: the one-shot fire — reached the due ⇒ clear it and suspend the heavy engines
/// (only SUSPENDABLE providers act). No re-arm until the next leaving-Review transition.
pub fn diag_idle_fire(editor: &mut Editor, now: u64) {
    if matches!(editor.diag_idle_due, Some(due) if now >= due) {
        editor.diag_idle_due = None;
        editor.diag_providers.suspend_all_idle_heavy();
    }
}
   ```
3. `app.rs::reduce` — extend the existing snapshot + single-exit tail:
   ```rust
    let before_id = editor.active().id;
    let before_version = editor.active().document.version;
    let before_summoned = crate::diagnostics_run::should_run_diagnostics(editor); // E10 §6
    let ctx = crate::overlays::DispatchCtx { reg, keymap, ex, clock, msg_tx, fs };
    let keep = reduce_dispatch(msg, editor, &ctx);
    crate::diagnostics_run::arm_if_edited(editor, before_id, before_version, clock);
    crate::diagnostics_run::idle_shutdown_track(editor, before_summoned, clock);
    keep
   ```
4. `timers.rs` — beside `diag_deadline`:
   ```rust
/// E10 §6: the idle-suspend deadline — armed by the leaving-Review transition, cleared on
/// re-entry/fire; `None` at rest (idle-free). The pos_sweep row's gated-Option shape.
fn diag_idle_deadline(e: &Editor, _now: u64) -> Option<u64> { e.diag_idle_due }
   ```
   `SUBSYSTEMS` gains (after the "diagnostics" row):
   `TimedSubsystem { name: "diag_idle", deadline: diag_idle_deadline },`
   and `on_tick`, after the `dispatch_diagnostics` block:
   ```rust
    // E10 §6: suspend the heavy engine when the idle deadline is reached.
    crate::diagnostics_run::diag_idle_fire(editor, now);
   ```

- [ ] **Step 4: Green, gate, commit**

```bash
cargo test --workspace 2>&1 | tail -3 && cargo clippy --workspace --all-targets 2>&1 | tail -2
git add wordcartel/src/editor.rs wordcartel/src/diagnostics_run.rs wordcartel/src/app.rs \
    wordcartel/src/timers.rs
git commit -m "feat: ltex idle-suspend app wiring — transition seam + timers row (E10 T8)"
```

---

### Task 9: Engine menu section + the warming status arm

**Files:**
- Modify: `wordcartel/src/diagnostics_run.rs` (rows fn), `wordcartel/src/menu.rs`
  (`DYNAMIC_SECTIONS`), `wordcartel/src/render_status.rs` (ONE new match arm — nothing else in
  the view layer)
- Test: `diagnostics_run.rs`, `menu.rs`, `render_status.rs` inline tests

**Interfaces:**
- Consumes: T7's `toggle_engine_*` command ids; `ProviderSet::{sources, is_enabled,
  availability}`; `menu::{DynamicSection, DYNAMIC_SECTIONS, MenuRowAction}`;
  `registry::{CommandId, MenuCategory}`.
- Produces: `diagnostics_run::engine_menu_rows(&Editor) -> Vec<(String, MenuRowAction)>`.

- [ ] **Step 1: Write the failing tests**

`diagnostics_run.rs::tests`:

```rust
    /// The COMPLETE spec-§11 label matrix — every cell of enabled×availability:
    /// disabled → "off" (wins over availability); enabled+Unavailable → "not installed";
    /// enabled+Starting → "warming…"; enabled+Ready → "on"; enabled+Idle → "on"; and the
    /// Plugin-source skip. Two fixture editors cover the five cells across three sources.
    #[test]
    fn engine_menu_rows_state_labels_and_toggle_actions() {
        use crate::diag_provider::{Availability, RecordingProvider};
        use crate::menu::MenuRowAction;
        use crate::registry::CommandId;
        /// A fixture editor with the three core engines at the given (availability, enabled).
        fn fixture(cells: [(DiagSource, Availability, bool); 3]) -> crate::editor::Editor {
            let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
            for (src, avail, enabled) in cells {
                e.diag_providers.install(Box::new(RecordingProvider::new()
                    .with_source(src).with_availability(avail)), enabled);
            }
            e
        }

        // Scenario A: on (Ready) / warming… / not installed — all ENABLED — plus Plugin skip.
        let mut e = fixture([
            (DiagSource::Harper, Availability::Ready, true),
            (DiagSource::LTeX, Availability::Starting, true),
            (DiagSource::Vale, Availability::Unavailable, true),
        ]);
        e.diag_providers.install(Box::new(RecordingProvider::new()
            .with_source(DiagSource::Plugin("mock"))), true); // skipped: no command (E12)
        let rows = engine_menu_rows(&e);
        assert_eq!(rows.len(), 3, "Plugin sources are skipped (spec §11)");
        assert_eq!(rows[0], ("Harper — on".to_string(),
            MenuRowAction::Command(CommandId("toggle_engine_harper"))));
        assert_eq!(rows[1], ("LTeX — warming…".to_string(),
            MenuRowAction::Command(CommandId("toggle_engine_ltex"))));
        assert_eq!(rows[2], ("vale — not installed".to_string(),
            MenuRowAction::Command(CommandId("toggle_engine_vale"))),
            "enabled + Unavailable → not installed");

        // Scenario B: the remaining cells — enabled+Idle → "on"; disabled → "off" (wins over
        // availability, here a Ready recorder).
        let e2 = fixture([
            (DiagSource::Harper, Availability::Idle, true),
            (DiagSource::LTeX, Availability::Ready, false),
            (DiagSource::Vale, Availability::Starting, false),
        ]);
        let rows2 = engine_menu_rows(&e2);
        assert_eq!(rows2[0].0, "Harper — on", "enabled + Idle (not yet summoned) → on");
        assert_eq!(rows2[1].0, "LTeX — off", "disabled wins over Ready availability");
        assert_eq!(rows2[2].0, "vale — off", "disabled wins over Starting availability");
    }
```

`menu.rs::tests` — test the REAL wiring seam directly. (Grounding: `grouped_commands` is the
build fn and it DOES consume the passed `&Editor` for dynamic rows — `for section in
DYNAMIC_SECTIONS { if section.category == cat { raw.extend((section.rows)(editor)…) } }` — but
the existing `group_items` test helper ignores its argument and rebuilds from a
`throwaway_editor()`, which has no providers installed. So assert on the `DYNAMIC_SECTIONS`
table itself and invoke its rows fn against a PREPARED editor — the exact seam
`grouped_commands` consumes; the label/action matrix is covered by the direct
`engine_menu_rows` test above):

```rust
    #[test]
    fn dynamic_sections_carries_the_view_engine_section() {
        // Fully-qualified: menu.rs::tests has only `use super::*;` (no Editor import) — the
        // neighbors (throwaway_editor etc.) all write crate::editor::Editor::new_from_text.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        crate::test_support::install_enabled_harper(&mut e);
        let section = DYNAMIC_SECTIONS.iter()
            .find(|s| s.category == MenuCategory::View)
            .expect("E10 §11: the engine section is registered under View");
        let rows = (section.rows)(&e);
        assert!(rows.iter().any(|(label, action)| label.starts_with("Harper — ")
                && *action == MenuRowAction::Command(CommandId("toggle_engine_harper"))),
            "the View dynamic section yields engine rows from the PASSED editor");
    }
```

`render_status.rs::tests` — **REWRITE the existing
`status_line_attributes_review_only_when_provider_ready` test** (it currently asserts
`Starting → "[REVIEW]"` with NO `·` — behavior E10 §12 deliberately changes, for harper too:
a brief harper Starting now shows the warming label). Replace it, renamed, with the full
matrix (the helper is `crate::render_status::status_left_text(&e)`; the mode segment renders
bracketed, so the exact strings are `[REVIEW · …]`):

```rust
    /// Effort A §10 + E10 §12: the Review attribution matrix. Ready → the engine label;
    /// Starting → the STEADY warming label (changed by E10 — pre-E10 Starting was plain);
    /// Idle / Unavailable → plain REVIEW, no attribution dot.
    #[test]
    fn status_line_review_attribution_matrix() {
        use crate::diag_provider::{RecordingProvider, Availability};
        use wordcartel_core::diagnostics::DiagSource;
        let with_availability = |a: Availability| {
            let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
            e.active_mut().view.mode = crate::editor::RenderMode::Review;
            e.diag_providers.install(Box::new(RecordingProvider::new()
                .with_source(DiagSource::Harper).with_availability(a)), true);
            crate::render_status::status_left_text(&e)
        };
        // The label comes from DiagSource::Harper.label(), not the provider's own identity.
        assert!(with_availability(Availability::Ready).contains("[REVIEW · Harper]"),
            "Ready → attribution");
        assert!(with_availability(Availability::Starting).contains("[REVIEW · warming Harper…]"),
            "Starting → the steady warming label (E10 §12)");
        for quiet in [Availability::Idle, Availability::Unavailable] {
            let s = with_availability(quiet);
            assert!(s.contains("[REVIEW]") && !s.contains("·"),
                "Idle/Unavailable → plain REVIEW, no attribution dot: {s}");
        }
    }
```

The no-entry state (empty `ProviderSet` → plain `[REVIEW]`) is already covered by
`status_line_shows_review_label`, which survives UNMODIFIED. The T9 commit is
intermediate-green: the old test is replaced and the new one passes in the same commit as the
implementation.

- [ ] **Step 2: Verify the reds** — compile FAIL (`engine_menu_rows` undefined); the
  render_status test FAILS (plain `REVIEW` today).

- [ ] **Step 3: Implement**

`diagnostics_run.rs` (production, near `install_core_providers`):

```rust
/// The engine-management dynamic menu rows (E10 §11): one row per registered engine,
/// state-in-label ("on" / "off" / "warming…" / "not installed"), dispatching that engine's
/// toggle command — menu ⊆ palette by construction. `Plugin` sources are skipped (their
/// rows are E12's plugin-contributed-menu effort). Availability is lazily discovered: an
/// absent binary reads "on" until Review first attempts a spawn (spec §11 display note).
pub fn engine_menu_rows(editor: &Editor) -> Vec<(String, crate::menu::MenuRowAction)> {
    use crate::diag_provider::Availability;
    editor.diag_providers.sources().filter_map(|src| {
        let cmd = match src {
            DiagSource::Harper => "toggle_engine_harper",
            DiagSource::LTeX => "toggle_engine_ltex",
            DiagSource::Vale => "toggle_engine_vale",
            DiagSource::Plugin(_) => return None,
        };
        let state = if !editor.diag_providers.is_enabled(src) { "off" }
            else {
                match editor.diag_providers.availability(src) {
                    Some(Availability::Unavailable) => "not installed",
                    Some(Availability::Starting) => "warming…",
                    _ => "on", // Idle | Ready | None-entry (unreachable for a listed source)
                }
            };
        Some((format!("{} — {}", src.label(), state),
            crate::menu::MenuRowAction::Command(crate::registry::CommandId(cmd))))
    }).collect()
}
```

`menu.rs` — the table gains a second row:

```rust
pub const DYNAMIC_SECTIONS: &[DynamicSection] = &[
    DynamicSection { category: MenuCategory::Documents, rows: crate::workspace::documents_menu_rows },
    // E10 §11: engine management ships with engines — a builtin fn-pointer row, zero
    // coupling to the deferred plugin-dynamic-menu machinery (that widening is E12's).
    DynamicSection { category: MenuCategory::View, rows: crate::diagnostics_run::engine_menu_rows },
];
```

`render_status.rs` — the Review arm becomes (complete; spec §12 matrix):

```rust
        crate::editor::RenderMode::Review => {
            let lens = editor.active_analysis_source;
            match editor.diag_providers.availability(lens) {
                // The label asserts a WORKING checker for the shown engine (SPINE §8.3)…
                Some(crate::diag_provider::Availability::Ready) =>
                    format!("REVIEW · {}", lens.label()).into(),
                // …and E10 §12 adds the steady warming state — render-derived, self-clearing
                // on the Starting→Ready flip, incapable of animating (no timer, no loop).
                Some(crate::diag_provider::Availability::Starting) =>
                    format!("REVIEW · warming {}…", lens.label()).into(),
                _ => "REVIEW".into(), // Idle / Unavailable / no entry: plain (unchanged)
            }
        }
```

`status_line_shows_review_label` (the EMPTY-ProviderSet → plain `[REVIEW]` case — spec §12's
"survives unmodified" sentence misattributes it as the Ready test; the Ready case lives in the
matrix test rewritten above) must pass UNMODIFIED.

- [ ] **Step 4: Green, gate, commit**

```bash
cargo test --workspace 2>&1 | tail -3 && cargo clippy --workspace --all-targets 2>&1 | tail -2
git add wordcartel/src/diagnostics_run.rs wordcartel/src/menu.rs wordcartel/src/render_status.rs
git commit -m "feat: engine-management View menu section + warming status arm (E10 T9)"
```

---

### Task 10: Default-engine seed override + PKGBUILD optdepends

**Files:**
- Modify: `wordcartel/src/diagnostics_run.rs` (`install_core_providers` tail),
  `packaging/arch/PKGBUILD.template` — the TRACKED source; `packaging/arch/PKGBUILD` is
  GENERATED and gitignored (`packaging/arch/.gitignore:11`) — do NOT edit it
- Test: `diagnostics_run.rs` inline tests

**Interfaces:**
- Consumes: T4's `DiagnosticsConfig.default_engine`; the existing first-enabled seed.
- Produces: the startup lens honoring `[diagnostics] default_engine`.

- [ ] **Step 1: Write the failing tests** (append to `diagnostics_run.rs::tests`, mirroring
  the T5 catalog test's arrange):

```rust
    #[test]
    fn default_engine_overrides_the_seed_when_enabled() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut warns = Vec::new();
        let mut cfg = crate::config::Config::default();
        cfg.diagnostics.default_engine = Some(DiagSource::LTeX);
        install_core_providers(&mut e, &cfg, &tx, &mut warns);
        assert_eq!(e.active_analysis_source, DiagSource::LTeX, "spec §13 override");
        assert!(warns.is_empty());
    }

    #[test]
    fn default_engine_disabled_falls_back_with_a_warning() {
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (40, 10));
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut warns = Vec::new();
        let mut cfg = crate::config::Config::default();
        cfg.diagnostics.default_engine = Some(DiagSource::Vale);
        cfg.diagnostics.linters = Some(vec!["harper".into(), "ltex".into()]); // vale NOT enabled
        install_core_providers(&mut e, &cfg, &tx, &mut warns);
        assert_eq!(e.active_analysis_source, DiagSource::Harper,
            "known-but-disabled → harper-first fallback (spec §13)");
        assert!(warns.iter().any(|w| w.contains("default_engine")),
            "the fallback is loud (config warning), never silent");
    }
```

- [ ] **Step 2: Verify the reds** — the first FAILS (seed stays Harper; the override doesn't
  exist yet).

- [ ] **Step 3: Implement** — in `install_core_providers`, directly after the existing
  first-enabled seed block (keep its comment):

```rust
    // E10 §13: the config-only default-engine override — applied ONLY when the named engine
    // is enabled; known-but-disabled falls back loudly. (Unknown NAMES were already rejected
    // at the config fold.) Direct field write, matching the seed above — construction, not
    // set_analysis_source (which would status-message).
    if let Some(want) = cfg.diagnostics.default_engine {
        if editor.diag_providers.is_enabled(want) {
            editor.active_analysis_source = want;
        } else {
            warns.push(format!(
                "config: diagnostics.default_engine — \"{}\" is not enabled; using {}",
                want.config_name(), editor.active_analysis_source.label()));
        }
    }
```

`packaging/arch/PKGBUILD.template` — its `optdepends=(` array carries the same entries as the
generated file (verified: the harper line reads
`'harper: grammar/spelling diagnostics in Review mode (harper-ls language server)'`); add
directly below that harper entry, same single-quoted `'pkg: reason'` format:

```bash
  'ltex-ls-plus: LanguageTool grammar/language diagnostics in Review mode (requires Java 21+)'
  'vale: prose style diagnostics in Review mode (the vale-ls backend CLI)'
  'vale-ls: prose style diagnostics in Review mode (Vale language server)'
```

- [ ] **Step 4: Green, gate, commit**

```bash
cargo test --workspace 2>&1 | tail -3 && cargo clippy --workspace --all-targets 2>&1 | tail -2
git add wordcartel/src/diagnostics_run.rs packaging/arch/PKGBUILD.template
git commit -m "feat: [diagnostics] default_engine seed override + optdepends (E10 T10)"
```

---

### Task 11: Live probe (mandatory-run, advisory-pass)

**Files:**
- Create: `scratchpad/lens-linting-arc/t11-probe-results.md` (the probe record — scratch, not
  committed; its VERBATIM summary lines go into the pre-merge report)
- No source changes expected. If the probe falsifies an empirical flag (spawn flags, the custom
  config request's shape, initializationOptions keys), the fix is a small engine-spec edit —
  report it as a FINDING to the controller first (it may touch spec §7/§8 text), then fix + a
  probe re-run.

**Prereqs:** `harper-ls`, `ltex-ls-plus` (Java 21+), `vale` + `vale-ls` installed on the probe
machine. If any is missing, record `SKIP — <binary> not installed` for its lines; a machine with
none records a full SKIP (still quoted in the pre-merge report — the suite is mandatory-RUN).

- [ ] **Step 1: Build + drive the real app** (use the `tui-interact` skill: private tmux
  session, send keys, capture screens):

```bash
cargo build 2>&1 | tail -2
```

Probe checklist (capture a screen per line; record PASS/FAIL/SKIP + the observed text):
1. Open a markdown file with a spelling error + a passive-voice sentence; enter Review
   (`F1`-cycle or the palette's Review command). Status shows `REVIEW · Harper`; harper
   underlines appear.
2. Palette → `Analysis Engine: LTeX`. Status shows `REVIEW · warming LTeX…` (steady — watch
   ~30 s: the label must NOT blink or animate) then `REVIEW · LTeX`; LanguageTool underlines
   appear. Record the observed warm duration.
3. Confirm ltex re-check after an edit lands within the steady watchdog (no false "no issues"
   blank mid-warm — the T2 machinery live).
4. View menu → the engine rows show `Harper — on`, `LTeX — on`, `vale — …` with live states.
5. Palette → `Analysis Engine: vale` (with a `.vale.ini` + styles in the cwd). Vale underlines
   appear; `vale-ls` did NOT download anything (`installVale:false` — check no network/`~/.vale`
   writes; run it once on a machine without `vale` to see the graceful
   `style linter unavailable…` hint).
6. Idle suspend: set `[diagnostics.ltex] idle_shutdown_min = 1`, enter Review (ltex active),
   leave Review, wait >1 min: `pgrep -f ltex` shows the JVM GONE. Re-enter Review: the JVM
   respawns, `warming LTeX…` shows, results return (resume = respawn path live).
7. Absent-binary hints: with `ltex-ls-plus` renamed away, entering Review on the LTeX lens
   shows the Java-21+ install hint once per Review entry.
8. `ltex/workspaceSpecificConfiguration`: grep the probe session's behavior — if ltex-ls-plus
   sent the custom request and rejected our response shape (errors in its stderr / no
   diagnostics ever), record the observed request/response JSON for the finding.

- [ ] **Step 2: Record + report**

Write each line's verdict to `scratchpad/lens-linting-arc/t11-probe-results.md` as
`probe: N/8 PASS (…SKIPs listed…)` plus per-line notes. Quote the summary line VERBATIM in the
effort's pre-merge report (advisory — a red probe line NEVER blocks the merge; it is surfaced to
the human explicitly, like the PTY smoke suite).

- [ ] **Step 3: Run the standing suites for the pre-merge report**

```bash
cargo test --workspace 2>&1 | tail -3
cargo clippy --workspace --all-targets 2>&1 | tail -2
scripts/smoke/run.sh 2>&1 | tail -1   # quote this line verbatim (mandatory-run, advisory)
```

No commit (scratch results only) unless a finding forced an engine-spec fix — then commit that
fix with its probe re-run noted in the message.

---

## Self-review (performed at authoring)

- **Spec coverage:** §3 → T1; §4 → T2; §5 → T3; §6 → T8; §7 → T5; §8 → T6; §9 → T4; §10 → T7;
  §11/§12 → T9; §13/§14.4 → T10; §15's T11 → T11. §14's zero-touch/E11/E8 boundaries are Global
  Constraints. No spec section is unimplemented.
- **Pin integrity:** T1 makes no test edits; the only harper-test edit in the whole plan is
  T4's `language: None` two-hunk diff, cross-checked by an explicit `git diff` review step.
- **Type consistency:** `suspend_all_idle_heavy` (T3 → T8), `engine_menu_rows` (T9 both files),
  `classify_spell_heuristic` (T5 → T6), `ProviderConfig.language` (T4 → T5/T6),
  `first_publish_seen` (T2 → T5 test), `diag_idle_due` (T8 both files) — names match at every
  use site.
- **Honest flags:** T2 Step 2 notes two of four tests pass immediately (regression guards, not
  reds); T1 is a no-red pin task by design; T4 is the pin's sanctioned edit; T5/T6 carry the
  three T11-probe empirical flags; test-helper names in T5/T7 defer to the neighboring tests'
  real local helpers (read-then-match, never invent a parallel harness). T9's seams are
  grounded exactly: `status_left_text` (render_status), the `DYNAMIC_SECTIONS` table (menu —
  the existing `group_items` helper ignores its argument and is unsuitable), and T9 REPLACES
  the pre-E10 `status_line_attributes_review_only_when_provider_ready` test because its
  Starting assertion describes behavior E10 §12 deliberately changed (spec §12's
  "survives unmodified" sentence misattributed the test inventory — the design matrix is
  unchanged; flagged as a finding, not a divergence).
