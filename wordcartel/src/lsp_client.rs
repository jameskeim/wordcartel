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
    /// Idle suspend-the-child eligibility (consumed by T3; ltex-only).
    const SUSPENDABLE: bool;
    /// E11 §2.3/T1: what a per-diagnostic fix request carries in `context.diagnostics` —
    /// `false` (default): the triple-matched raw alone; `true`: EVERY retained raw from the
    /// same publish (the shape ltex demonstrably answers in batch; range stays the matched
    /// raw's own). Flipped per engine by T1's probe outcome.
    const FIX_CONTEXT_ALL_RAWS: bool = false;
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
    /// E11 §4: is this CodeAction `kind` a FIX this engine delivers as an edit?
    /// Probe-grounded per engine — command-only kinds are excluded by knowledge, not luck.
    fn is_fix_kind(kind: &str) -> bool;
}

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

/// Grace after `shutdown` before the pump forces `exit` + kills the child (bounded quit latency).
const SHUTDOWN_GRACE_MS: u64 = 1_000;
/// Respawn budget per session — the initial spawn counts as the first (spec §3.4; anti-crash-loop).
const MAX_SPAWN_ATTEMPTS: u32 = 3;
/// E11 §3.3: the flat leash on ONE on-demand fix request, covering HELD + sent time. Live from
/// the moment the slot is materialized, in every phase — during a long JVM warm the writer gets
/// an honest "no fixes available" within 10 s and can reopen to retry, rather than watching
/// "fetching…" for minutes (a warm-aware deadline was considered and rejected for that reason).
const FIX_REQUEST_TIMEOUT_MS: u64 = 10_000;

/// A command from the app-side handle, delivered over the `Inbound` channel.
#[derive(Debug, Clone)]
pub(crate) enum Cmd {
    Configure(ProviderConfig),
    Change { buffer_id: BufferId, version: u64, path: Option<std::path::PathBuf>, text: String },
    Close { buffer_id: BufferId },
    ReloadDict,
    Shutdown,
    /// E10 §5: ask a heavy engine to release its child process until next summoned.
    Suspend,
    /// E11 §3.2: fetch fix candidates for ONE diagnostic, on demand (quick-fix overlay open).
    /// `token` is the correlation identity; `code`/`message` ride because a byte range alone
    /// cannot deterministically pick the raw diagnostic to echo (overlapping anchors exist).
    /// Produced solely by `LspProvider::request_fixes` (T5) — the app-side handle's seam.
    RequestFixes { token: u64, buffer_id: BufferId, version: u64,
        range: std::ops::Range<usize>, code: Option<String>, message: String },
}

/// Everything the pump receives: app commands, one parsed server frame, or reader end-of-stream.
pub(crate) enum Inbound {
    Cmd(Cmd),
    Server(Value),
    ServerEof,
}

/// A side effect the pump performs on the state machine's behalf. `ClientState` returns these; the
/// thread executes them (never the reverse) so all protocol logic stays pure.
pub(crate) enum Action {
    Send(Value),
    Emit(Msg),
    SetAvailability(Availability),
    Respawn,
    Exit,
    /// E10 §5: kill the child, keep the client thread parked (a blocked thread is free).
    Park,
    /// E10 §5: resume via the ordinary respawn path — WITHOUT consuming the crash-respawn budget.
    Unpark,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase { Initializing, Running, ShuttingDown, Suspended }

/// Per-document server-sync bookkeeping. `text` is the exact string last sent for this generation —
/// LSP positions are converted against it, never the live buffer (spec §5, §6).
pub(crate) struct DocState {
    pub(crate) uri: String, pub(crate) lsp_version: i32, pub(crate) our_version: u64,
    pub(crate) generation: u64, pub(crate) text: String, pub(crate) open: bool,
    /// The raw LSP diagnostics array from the most recent publish for this doc — the on-demand
    /// quick-fix fetch's source (E11 §3.3). STORED IN LOCKSTEP WITH THE DIAGNOSTICS STORE: every
    /// attributed publish writes it, tagged with the SAME version that publish's
    /// `DiagnosticsDone` carries. What the writer sees underlined and what the fix fetch echoes
    /// are therefore the same publish, by construction (E11 T10 — see `on_publish`).
    pub(crate) last_raw: Option<(u64, Vec<Value>)>,
}
/// A didOpen/didChange awaiting its `publishDiagnostics` (or the publish watchdog).
pub(crate) struct AwaitPublish { pub(crate) our_version: u64, pub(crate) generation: u64, pub(crate) deadline: u64 }
/// The ONE live on-demand fix request (E11 §3.3) — the overlay is XOR-single, so at most one can
/// exist. The slot IS this command's queue: it is materialized in `on_inbound` in every phase and
/// never enters `queued`, so its `deadline` is visible to `next_deadline()` from acceptance.
pub(crate) struct PendingFix {
    pub(crate) token: u64,
    pub(crate) buffer_id: BufferId,
    pub(crate) version: u64,
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) code: Option<String>,
    pub(crate) message: String,
    pub(crate) deadline: u64,
    /// `Some` once the wire request went out — the `pending_requests` key to de-register when
    /// any empty-terminal leg resolves the slot (a late response then hits the unknown-id arm).
    pub(crate) sent_id: Option<u64>,
}
/// What an outstanding JSON-RPC request id means when its response lands.
pub(crate) enum PendingKind {
    Initialize, Shutdown,
    /// E11 §3.3: the per-diagnostic `textDocument/codeAction` in flight. The token is IN the
    /// variant — correlation must never route back through mutable state.
    FixRequest { token: u64, buffer_id: BufferId, generation: u64, version: u64,
        range: std::ops::Range<usize> },
}

/// The pure protocol state machine (spec §3.3), parameterized over the engine `E`. No IO — feed it
/// `Inbound` + `now_ms`, execute the returned `Vec<Action>`. Exhaustively unit-testable (see harper's
/// inline tests, which drive `ClientState<HarperEngine>`).
pub(crate) struct ClientState<E: LspEngine> {
    pub(crate) phase: Phase,
    pub(crate) cfg: ProviderConfig,
    pub(crate) docs: HashMap<BufferId, DocState>,
    pub(crate) uri_owner: HashMap<String, (BufferId, u64)>,
    pub(crate) next_generation: u64,
    pub(crate) queued: Vec<Cmd>,
    pub(crate) next_id: u64,
    pub(crate) pending_requests: HashMap<u64, PendingKind>,
    pub(crate) awaiting_publish: HashMap<BufferId, AwaitPublish>,
    /// E11 §3.3: the single on-demand fix request slot (see [`PendingFix`]).
    pub(crate) pending_fix: Option<PendingFix>,
    pub(crate) spawn_attempts: u32,
    /// True once this child process has produced its first owned-URI publish (E10 §4) — gates
    /// which watchdog deadline `on_change` stamps. Reset in `on_spawned` so a respawned child
    /// re-enters the warm phase.
    pub(crate) first_publish_seen: bool,
    /// EOFs we have PRE-DECLARED by deliberately killing a live child (the `Cmd::Suspend`
    /// handler's `Action::Park` — the sole increment site): each such child's reader will
    /// deliver exactly one `ServerEof`, which must be DRAINED even if it arrives after a
    /// resume has already moved the phase past `Suspended` (the change-before-EOF race — an
    /// edit landing on the FIFO channel ahead of the old reader's EOF). A counter, not a
    /// bool: rapid suspend→resume→suspend cycles can leave TWO deliberate-kill EOFs
    /// logically outstanding (each reader thread delivers on its own schedule). Crash EOFs
    /// and the synthetic respawn-fail EOF never touch this — they must keep consuming the
    /// respawn budget in `on_server_gone`.
    pub(crate) expected_eofs: u32,
    pub(crate) _engine: std::marker::PhantomData<E>,
}

impl<E: LspEngine> ClientState<E> {
    /// A fresh machine, pre-handshake. `spawn_attempts` starts at 1 — the initial spawn counts.
    pub(crate) fn new(cfg: ProviderConfig) -> Self {
        ClientState {
            phase: Phase::Initializing, cfg, docs: HashMap::new(), uri_owner: HashMap::new(),
            next_generation: 1, queued: Vec::new(), next_id: 1, pending_requests: HashMap::new(),
            awaiting_publish: HashMap::new(), pending_fix: None, spawn_attempts: 1,
            first_publish_seen: false, expected_eofs: 0,
            _engine: std::marker::PhantomData,
        }
    }

    fn alloc_id(&mut self) -> u64 { let id = self.next_id; self.next_id += 1; id }

    /// True once `Cmd::Shutdown` was applied — the pump arms its grace timer off this.
    pub(crate) fn is_shutting_down(&self) -> bool { self.phase == Phase::ShuttingDown }

    /// The soonest watchdog deadline, if any — the pump's `recv_timeout` bound (idle = `None`).
    /// Chains the `pending_fix` leash so a HELD slot (any phase, including mid-JVM-warm) is
    /// visible to the pump from the moment it was accepted (E11 §3.3).
    pub(crate) fn next_deadline(&self) -> Option<u64> {
        merge_deadline(self.awaiting_publish.values().map(|a| a.deadline).min(),
            self.pending_fix.as_ref().map(|p| p.deadline))
    }

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

    /// (Re)spawn handshake step: reset to `Initializing`, mark every doc closed, clear pending, and
    /// send `initialize`. The pump must read its RESPONSE before `initialized` (deadlock guard).
    pub(crate) fn on_spawned(&mut self, _now: u64) -> Vec<Action> {
        self.first_publish_seen = false;
        self.phase = Phase::Initializing;
        for d in self.docs.values_mut() { d.open = false; }
        // That loop retires every open URI, so the ownership map goes with it. Load-bearing on the
        // SUSPEND→UNPARK resume path, which deliberately bypasses `on_server_gone` (the
        // expected-EOF drain) and so would otherwise strand the pre-suspend URI's entry every idle
        // cycle; harmless on the crash path, where `on_server_gone` has already cleared. With it
        // the no-leak invariant is uniform across all three reopen paths.
        self.uri_owner.clear();
        self.pending_requests.clear();
        let id = self.alloc_id();
        self.pending_requests.insert(id, PendingKind::Initialize);
        vec![Action::Send(self.initialize_request(id))]
    }

    /// The top-level router (spec §3.3).
    pub(crate) fn on_inbound(&mut self, inb: Inbound, now: u64) -> Vec<Action> {
        match inb {
            Inbound::Cmd(c) => {
                // E11 §3.3: `RequestFixes` gets FIRST-CLASS routing ahead of the generic queue
                // arm (the `Cmd::Suspend` precedent) and is NEVER pushed onto `queued` — the
                // queue holds bare `Cmd`s that `next_deadline()` cannot see, so a queue-side
                // request during a JVM warm would get its leash only after initialization
                // (minutes of visible "fetching"). The slot IS this command's queue.
                let c = match c {
                    Cmd::RequestFixes { token, buffer_id, version, range, code, message } =>
                        return self.on_request_fixes(PendingFix { token, buffer_id, version,
                            range, code, message, deadline: now + FIX_REQUEST_TIMEOUT_MS,
                            sent_id: None }),
                    other => other,
                };
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
                    // Pre-handshake: queue for replay. Configure only updates cfg (the handshake's
                    // didChangeConfiguration carries it) so it never double-applies.
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
            // EXPECTED — drained, never routed to the crash/respawn path. The COUNTER is
            // what closes the resume race: the stale EOF may arrive only after an Unpark
            // has already moved the phase to Initializing (change-before-EOF on the FIFO
            // channel), where a phase-only check would mis-route it to `on_server_gone`
            // (budget burned + the queued post-resume check falsely flushed). The phase
            // check stays as a parked-window backstop: while Suspended no child exists, so
            // no EOF there can be a live-child crash.
            Inbound::ServerEof => {
                if self.expected_eofs > 0 { self.expected_eofs -= 1; Vec::new() }
                else if self.phase == Phase::Suspended { Vec::new() }
                else { self.on_server_gone(now) }
            }
        }
    }

    fn apply_cmd(&mut self, c: Cmd, now: u64) -> Vec<Action> {
        match c {
            // E11 §3.3: intercepted in `on_inbound` in every phase, so it never reaches the
            // apply/replay path — and it is never in `queued` for the replay to hand back.
            Cmd::RequestFixes { .. } => {
                debug_assert!(false, "RequestFixes is routed in on_inbound, never applied/queued");
                Vec::new()
            }
            Cmd::Change { buffer_id, version, path, text } =>
                self.on_change(buffer_id, version, path, text, now),
            Cmd::Close { buffer_id } => self.on_close(buffer_id),
            Cmd::ReloadDict => self.settings_push_action().into_iter().collect(),
            Cmd::Configure(cfg) => { self.cfg = cfg; self.settings_push_action().into_iter().collect() }
            Cmd::Suspend => {
                // Running only (on_inbound drops it otherwise). Flush-first makes the
                // terminal guarantee unconditional; then best-effort polite teardown —
                // fire-and-forget: NO PendingKind, a late response hits the unknown-id arm.
                let mut out = self.flush_outstanding();
                let id = self.alloc_id();
                out.push(Action::Send(json!({"jsonrpc":"2.0","id":id,"method":"shutdown"})));
                out.push(Action::Send(json!({"jsonrpc":"2.0","method":"exit"})));
                out.push(Action::Park);
                // Pre-declare the killed child's EOF (SOLE increment site): Running ⟺ a live
                // child, and Park kills it — its reader will deliver exactly one ServerEof,
                // possibly only after a resume has already left `Suspended` (the race).
                self.expected_eofs += 1;
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
        }
    }

    /// A full-document sync. Records the awaiting slot FIRST (the accepted-but-unrecorded latch
    /// guard, spec §3.2/§5.1) — before any `Send` — so a mid-send death still flushes a terminal.
    fn on_change(&mut self, buffer_id: BufferId, version: u64,
        _path: Option<std::path::PathBuf>, text: String, now: u64) -> Vec<Action> {
        // E10 §4: until this child's first publish, the watchdog runs at the engine's
        // warm-phase deadline (JVM boot + model load land in first-CHECK latency).
        let publish_timeout = if self.first_publish_seen { E::PUBLISH_TIMEOUT_MS }
            else { E::FIRST_CHECK_TIMEOUT_MS.unwrap_or(E::PUBLISH_TIMEOUT_MS) };
        let reopen = !self.docs.get(&buffer_id).map(|d| d.open).unwrap_or(false);
        // E11 §3.3 change-invalidation: an edit PAST a pending slot's snapshot retires it now —
        // the fix target belongs to a superseded version, and the overlay's own `opened_version`
        // guard would refuse the apply anyway. Killing it here (rather than leaning on the
        // downstream guard) means any surviving slot satisfies `our_version == version`.
        let stale_slot = self.pending_fix.as_ref()
            .is_some_and(|p| p.buffer_id == buffer_id && p.version < version);
        let mut out: Vec<Action> = if stale_slot {
            self.resolve_pending_fix_empty().into_iter().collect()
        } else { Vec::new() };
        if reopen {
            let generation = self.next_generation; self.next_generation += 1;
            let uri = crate::lsp_rpc::doc_uri(buffer_id, generation);
            self.uri_owner.insert(uri.clone(), (buffer_id, generation));
            let lsp_version = 1;
            // Record awaiting BEFORE the Send action (non-IO first step; flush covers a mid-send death).
            self.awaiting_publish.insert(buffer_id,
                AwaitPublish { our_version: version, generation, deadline: now + publish_timeout });
            out.push(Action::Send(json!({
                "jsonrpc":"2.0","method":"textDocument/didOpen",
                "params":{"textDocument":{"uri":uri,"languageId": E::LANGUAGE_ID,"version":lsp_version,"text":text}}})));
            self.docs.insert(buffer_id,
                DocState { uri, lsp_version, our_version: version, generation, text, open: true,
                    last_raw: None });
        } else {
            let (uri, generation, lsp_version) = {
                let d = self.docs.get_mut(&buffer_id).expect("open doc exists");
                d.lsp_version = d.lsp_version.saturating_add(1);
                debug_assert!(d.lsp_version < i32::MAX, "lsp_version overflow");
                d.our_version = version; d.text = text.clone();
                (d.uri.clone(), d.generation, d.lsp_version)
            };
            self.awaiting_publish.insert(buffer_id,
                AwaitPublish { our_version: version, generation, deadline: now + publish_timeout });
            out.push(Action::Send(json!({
                "jsonrpc":"2.0","method":"textDocument/didChange",
                "params":{"textDocument":{"uri":uri,"version":lsp_version},
                    "contentChanges":[{"text":text}]}})));
        }
        out
    }

    /// `Cmd::Close`: **emit the terminal FIRST, then remove state** (spec §3.3, round-3 #2) — so a
    /// latched in-flight version is guaranteed its terminal and no `flush_outstanding` can re-emit.
    fn on_close(&mut self, buffer_id: BufferId) -> Vec<Action> {
        let mut out = Vec::new();
        let outstanding = self.awaiting_publish.remove(&buffer_id).map(|a| a.our_version);
        if let Some(version) = outstanding {
            out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id, version,
                source: E::SOURCE, diagnostics: Vec::new() }));
        }
        // E11 §3.3 document-close leg: resolve a matching slot BEFORE state removal (the
        // terminal-first house pattern). Without it, a response landing after the close would
        // consume its `PendingKind`, be dropped as stale, and leave the accepted request with
        // NO terminal — the one leg the round-2 exactly-once matrix missed.
        if self.pending_fix.as_ref().is_some_and(|p| p.buffer_id == buffer_id) {
            out.extend(self.resolve_pending_fix_empty());
        }
        if let Some(d) = self.docs.remove(&buffer_id) {
            self.uri_owner.remove(&d.uri);
            // The wire frame ONLY when the doc is actually open: a §3.3 timeout-RETIRED doc is
            // still present with `open = false`, its URI already didClosed at retirement, and an
            // unconditional send here would didClose that retired URI a second time.
            if d.open {
                out.push(Action::Send(json!({"jsonrpc":"2.0","method":"textDocument/didClose",
                    "params":{"textDocument":{"uri":d.uri}}})));
            }
        }
        out
    }

    /// Route one server frame: request (has `id` + `method`), notification (`method`, no `id`), or
    /// response (`id`, no `method`).
    fn on_server(&mut self, v: Value, now: u64) -> Vec<Action> {
        let has_method = v.get("method").is_some();
        let has_id = v.get("id").map(|i| !i.is_null()).unwrap_or(false);
        if has_method && has_id { self.on_server_request(&v) }
        else if has_method { self.on_server_notification(v, now) }
        else { self.on_server_response(v, now) }
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

    fn on_server_notification(&mut self, v: Value, now: u64) -> Vec<Action> {
        match v["method"].as_str().unwrap_or("") {
            "textDocument/publishDiagnostics" => self.on_publish(&v, now),
            _ => Vec::new(),
        }
    }

    fn on_server_response(&mut self, v: Value, now: u64) -> Vec<Action> {
        let kind = v["id"].as_u64().and_then(|i| self.pending_requests.remove(&i));
        match kind {
            Some(PendingKind::Initialize) => self.on_initialized(now),
            Some(PendingKind::Shutdown) =>
                vec![Action::Send(json!({"jsonrpc":"2.0","method":"exit"})), Action::Exit],
            Some(PendingKind::FixRequest { token, buffer_id, generation, version, range }) =>
                self.on_fix_response(token, buffer_id, generation, version, range, &v),
            None => Vec::new(),
        }
    }

    /// The `initialize` RESPONSE landed — NOW it is safe to send `initialized` (deadlock guard).
    /// Then push config (the re-pull trigger), go `Running`, and replay queued commands in order.
    fn on_initialized(&mut self, now: u64) -> Vec<Action> {
        let mut out = vec![
            Action::Send(json!({"jsonrpc":"2.0","method":"initialized","params":{}})),
        ];
        out.extend(self.settings_push_action());
        // Handshake complete → the provider is LIVE (spec §10). This is the sole production
        // Ready transition: it lets `render_status` attribute `REVIEW · Harper` and stops the
        // debounced-recheck path stamping a permanent "starting grammar checker…". The SAME
        // path runs after a crash+respawn's re-initialize, so Ready is RESTORED post-respawn
        // (clearing the transient Starting stamped by `on_server_gone`).
        out.push(Action::SetAvailability(Availability::Ready));
        self.phase = Phase::Running;
        for c in std::mem::take(&mut self.queued) { out.extend(self.apply_cmd(c, now)); }
        // E11 §3.3 attempt site (b): a slot held across the handshake may have become sendable
        // now that the queue replayed and the machine is Running.
        out.extend(self.try_send_fix());
        out
    }

    /// A `publishDiagnostics` notification. URI-keyed generation attribution (spec §3.3 Receive):
    /// an absent uri → drop; otherwise emit the converted set IMMEDIATELY, suggestionless — the
    /// batched codeAction round trip is gone (E11 §3): fixes are fetched on-demand when the
    /// writer opens the quick-fix overlay (a later task), never blocking paint.
    fn on_publish(&mut self, v: &Value, _now: u64) -> Vec<Action> {
        let params = &v["params"];
        let uri = match params["uri"].as_str() { Some(u) => u.to_string(), None => return Vec::new() };
        let (buffer_id, generation) = match self.uri_owner.get(&uri) {
            Some(&pair) => pair, None => return Vec::new(), // closed / superseded generation → drop
        };
        let (tagged, text, lsp_version) = match self.docs.get(&buffer_id) {
            Some(d) if d.open && d.generation == generation =>
                (d.our_version, d.text.clone(), d.lsp_version),
            _ => return Vec::new(),
        };
        // An attributed publish proves the engine warm even when the version-echo mismatches
        // below (E10 §4) — the watchdog only needs proof the child is alive and talking.
        self.first_publish_seen = true;
        // Secondary in-generation guard: drop a stale snapshot when the echo IS present (harper 2.1.0
        // omits it — generation is the load-bearing tag; this never blocks the omitted case).
        if let Some(ver) = params.get("version").and_then(|x| x.as_i64()) {
            if ver != lsp_version as i64 { return Vec::new(); }
        }
        let raw: Vec<Value> = params["diagnostics"].as_array().cloned().unwrap_or_default();
        let converted = self.convert_diagnostics(&raw, &text);
        // E11 §3.3 LOCKSTEP ATTRIBUTION (T10 live fix, replacing the await-attribution rule): the
        // raws are stored on EVERY attributed publish, tagged with `tagged` — the very version
        // this publish's `DiagnosticsDone` carries just below. The store and the echo source thus
        // move together by construction. The retired rule tagged from the ANSWERED AWAIT instead,
        // which is equivalent only while publishes and awaits stay 1:1: harper emits exactly one
        // publish per didChange, ltex does NOT. A straggler from the previous check landing inside
        // the next check's await window made the store take one publish and `last_raw` take
        // another — permanently disagreeing at the same version tag, so `raw_matches` found no
        // triple match and the fetch resolved empty for precisely the diagnostic the writer had
        // just edited (probe F1: apply a fix → undo → `(no fixes available)` forever).
        if let Some(d) = self.docs.get_mut(&buffer_id) { d.last_raw = Some((tagged, raw)); }
        // The publish arrived; retire the await slot. Its generation must match the URI-attributed
        // one (both are stamped from the same reopen) — a soundness cross-check on attribution.
        if let Some(a) = self.awaiting_publish.remove(&buffer_id) {
            debug_assert_eq!(a.generation, generation, "awaiting generation matches attributed publish");
        }
        let mut out = vec![Action::Emit(Msg::DiagnosticsDone { buffer_id, version: tagged,
            source: E::SOURCE, diagnostics: converted })];
        // E11 §3.3 attempt site (c): a fresh tagged `last_raw` is the moment a held slot's send
        // condition can newly become true.
        out.extend(self.try_send_fix());
        out
    }

    /// Convert an LSP diagnostics array to our byte-ranged set against `text` (spec §6/§7). Drops
    /// unconvertible ranges and — when `!cfg.grammar` — Grammar-classified diagnostics.
    fn convert_diagnostics(&self, raw: &[Value], text: &str) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for d in raw {
            let range = match raw_byte_range(d, text) { Some(r) => r, None => continue };
            let kind = E::classify(d);
            if !self.cfg.grammar && kind == DiagnosticKind::Grammar { continue; }
            let message = raw_message(d).to_string();
            let code = raw_code(d);
            let href = d.get("codeDescription").and_then(|c| c.get("href"))
                .and_then(|h| h.as_str()).map(str::to_string);
            out.push(Diagnostic { range, kind, source: E::SOURCE, code, href, message,
                suggestions: Vec::new() });
        }
        out.sort_by_key(|d| d.range.start);
        out
    }

    /// The publish watchdog (spec §3.4): removes the tracked entry BEFORE emitting
    /// (terminal-guarantee) — a publish past deadline emits an empty terminal so the
    /// single-in-flight latch never wedges.
    pub(crate) fn on_deadline(&mut self, now: u64) -> Vec<Action> {
        let mut out = Vec::new();
        let expired_pub: Vec<BufferId> = self.awaiting_publish.iter()
            .filter(|(_, a)| now >= a.deadline).map(|(b, _)| *b).collect();
        for b in expired_pub {
            if let Some(a) = self.awaiting_publish.remove(&b) {
                out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id: b, version: a.our_version,
                    source: E::SOURCE, diagnostics: Vec::new() }));
            }
            out.extend(self.retire_generation(b));
        }
        // E11 §3.3: the slot's own leash. A slot that never became sendable expires here into an
        // honest empty terminal — no infinite hold.
        if self.pending_fix.as_ref().is_some_and(|p| now >= p.deadline) {
            out.extend(self.resolve_pending_fix_empty());
        }
        out
    }

    /// E11 §3.3 rule 1 — RETIRE a timed-out document generation without leaking it. A publish
    /// timeout is not a crash: the child is alive, so the obsolete document is closed on the wire
    /// too. Removing the `uri_owner` mapping is what makes a late publish for the timed-out change
    /// drop WHOLE at the attribution lookup — never converted, never stored, never tagged; and it
    /// is what keeps `uri_owner`/the server's open-document set from growing one entry per timeout
    /// (the shipped reopen branch only INSERTS, because its only prior caller was a respawn where
    /// `on_server_gone` clears the map wholesale and the dead server needs no didClose).
    fn retire_generation(&mut self, buffer_id: BufferId) -> Vec<Action> {
        let uri = match self.docs.get_mut(&buffer_id) {
            Some(d) if d.open => { d.open = false; d.last_raw = None; d.uri.clone() }
            _ => return Vec::new(),
        };
        self.uri_owner.remove(&uri);
        vec![Action::Send(json!({"jsonrpc":"2.0","method":"textDocument/didClose",
            "params":{"textDocument":{"uri":uri}}}))]
    }

    /// The server is gone (EOF / write error / corrupt frame). **Flush all outstanding FIRST** — the
    /// round-1 CRITICAL wedge guard — then respawn (budget remaining) or degrade (spec §3.4).
    pub(crate) fn on_server_gone(&mut self, _now: u64) -> Vec<Action> {
        let mut out = self.flush_outstanding();
        if self.spawn_attempts < MAX_SPAWN_ATTEMPTS {
            self.spawn_attempts += 1;
            self.phase = Phase::Initializing;
            for d in self.docs.values_mut() { d.open = false; }
            self.uri_owner.clear();
            self.pending_requests.clear();
            out.push(Action::SetAvailability(Availability::Starting));
            out.push(Action::Emit(Msg::DiagProviderEvent { source: E::SOURCE,
                event: ProviderEvent::Restarted }));
            out.push(Action::Respawn);
        } else {
            out.push(Action::SetAvailability(Availability::Unavailable));
            out.push(Action::Emit(Msg::DiagProviderEvent { source: E::SOURCE,
                event: ProviderEvent::Degraded(E::CRASHED_HINT.into()) }));
            out.push(Action::Exit);
        }
        out
    }

    /// Drain-as-it-emits: an empty version-tagged terminal for every entry STILL tracked in
    /// `awaiting_publish` + queued `Cmd::Change`, removing each as it emits. Idempotent (a second
    /// call emits nothing) — the FlushGuard's drop can call it after `on_server_gone` did.
    pub(crate) fn flush_outstanding(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        for (b, a) in self.awaiting_publish.drain() {
            out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id: b, version: a.our_version,
                source: E::SOURCE, diagnostics: Vec::new() }));
        }
        for c in std::mem::take(&mut self.queued) {
            if let Cmd::Change { buffer_id, version, .. } = c {
                out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id, version,
                    source: E::SOURCE, diagnostics: Vec::new() }));
            }
        }
        // E11 §3.4: the slot is the third flush track — thread death or a server EOF between
        // accept and apply must not strand a Fetching overlay.
        out.extend(self.resolve_pending_fix_empty());
        out
    }

    /// Resolve the live slot with its EMPTY terminal: take it, de-register any in-flight id (a
    /// late response then falls to the existing unknown-id arm — no double emission), and emit
    /// the token's `DiagFixesReady`. THE single funnel for every empty leg of exactly-once —
    /// replacement, change-invalidation, no-triple-match, deadline, close, and both flushes.
    fn resolve_pending_fix_empty(&mut self) -> Option<Action> {
        let p = self.pending_fix.take()?;
        if let Some(id) = p.sent_id { self.pending_requests.remove(&id); }
        Some(Action::Emit(Msg::DiagFixesReady { token: p.token, buffer_id: p.buffer_id,
            version: p.version, source: E::SOURCE, range: p.range, suggestions: Vec::new() }))
    }

    /// `Cmd::RequestFixes` (E11 §3.3): materialize the slot NOW, in whatever phase, then attempt
    /// the send. Replacing a live slot terminates the DISPLACED request first — exactly-once holds
    /// universally, not just for the survivor; the displaced terminal is dropped harmlessly by the
    /// reduce-side token guard (its overlay is gone).
    fn on_request_fixes(&mut self, slot: PendingFix) -> Vec<Action> {
        let mut out: Vec<Action> = self.resolve_pending_fix_empty().into_iter().collect();
        self.pending_fix = Some(slot);
        out.extend(self.try_send_fix()); // attempt site (a): the conditions may already hold
        out
    }

    /// Evaluate the §3.3 send condition against the CURRENT state — read-only, so the decision
    /// and the mutation stay separable (`try_send_fix` performs whichever it returns).
    fn evaluate_fix_send(&self) -> FixAttempt {
        let p = match &self.pending_fix {
            // Already on the wire: the attempt sites re-fire on every publish/replay, and a
            // second send would orphan the first id's `PendingKind` entry.
            Some(p) if p.sent_id.is_none() => p,
            _ => return FixAttempt::Hold,
        };
        if self.phase != Phase::Running { return FixAttempt::Hold; }
        let d = match self.docs.get(&p.buffer_id) {
            Some(d) if d.open => d, _ => return FixAttempt::Hold,
        };
        // ONE snapshot: the text we converted against, the raws we echo, and the request's own
        // version must all name `PendingFix.version` — otherwise the three artifacts disagree.
        let raws = match &d.last_raw {
            Some((tag, raws)) if *tag == p.version && d.our_version == p.version => raws,
            _ => return FixAttempt::Hold,
        };
        match raws.iter().find(|r| raw_matches(r, &d.text, &p.range, &p.code, &p.message)) {
            // The wire request is materialized HERE, at send time — never at slot creation, whose
            // URI would name a generation retired by an intervening retirement or resume. `range`
            // is the matched raw's own verbatim wire range: the exact positions the server itself
            // published, which sidesteps a byte→UTF-16 inverse conversion entirely.
            Some(m) => FixAttempt::Send {
                uri: d.uri.clone(), generation: d.generation, wire_range: m["range"].clone(),
                context: if E::FIX_CONTEXT_ALL_RAWS { raws.clone() } else { vec![m.clone()] },
            },
            // Version-matched raws present but nothing carries this identity: the fix target no
            // longer exists server-side, so the slot resolves NOW rather than waiting out
            // its leash.
            None => FixAttempt::ResolveEmpty,
        }
    }

    /// Attempt the held slot's send (E11 §3.3). Called at the three moments the send condition can
    /// newly become true: slot creation (a), the `on_initialized` queue replay (b), and a publish
    /// that stores a fresh tagged `last_raw` (c).
    fn try_send_fix(&mut self) -> Vec<Action> {
        match self.evaluate_fix_send() {
            FixAttempt::Hold => Vec::new(),
            FixAttempt::ResolveEmpty => self.resolve_pending_fix_empty().into_iter().collect(),
            FixAttempt::Send { uri, generation, wire_range, context } => {
                let id = self.alloc_id();
                let (token, buffer_id, version, range) = {
                    let p = self.pending_fix.as_mut()
                        .expect("slot present — evaluate_fix_send just read it");
                    p.sent_id = Some(id);
                    (p.token, p.buffer_id, p.version, p.range.clone())
                };
                self.pending_requests.insert(id,
                    PendingKind::FixRequest { token, buffer_id, generation, version, range });
                vec![Action::Send(json!({"jsonrpc":"2.0","id":id,"method":"textDocument/codeAction",
                    "params":{"textDocument":{"uri":uri},"range":wire_range,
                        "context":{"diagnostics":context}}}))]
            }
        }
    }

    /// The per-diagnostic `codeAction` RESPONSE (E11 §3.4). Emits `DiagFixesReady` with EVERY
    /// matched suggestion (possibly empty) and clears the slot.
    ///
    /// A slot that is gone or now belongs to a different token was ALREADY terminated
    /// (replacement/close/deadline each de-register the sent id, so this is belt-and-braces).
    /// A STALE GENERATION *for the live slot* resolves it EMPTY, immediately: this response
    /// consumed the request's `pending_requests` entry, so nothing can ever answer that request
    /// again — waiting out its 10 s leash would be a knowingly futile "fetching…". The immediate
    /// empty resolution IS the token's one terminal.
    fn on_fix_response(&mut self, token: u64, buffer_id: BufferId, generation: u64,
        version: u64, range: std::ops::Range<usize>, v: &Value) -> Vec<Action> {
        match &self.pending_fix {
            Some(p) if p.token == token => {
                debug_assert_eq!(p.version, version, "response identity matches the live slot");
                debug_assert_eq!(p.range, range, "response anchor matches the live slot");
            }
            _ => return Vec::new(),
        }
        let fresh = match self.docs.get(&buffer_id) {
            Some(d) if d.generation == generation => Some((d.uri.clone(), d.text.clone())),
            _ => None,
        };
        let (uri, text) = match fresh {
            Some(pair) => pair,
            None => return self.resolve_pending_fix_empty().into_iter().collect(),
        };
        let empty: Vec<Value> = Vec::new();
        let actions = v.get("result").and_then(|r| r.as_array()).unwrap_or(&empty);
        let suggestions = crate::lsp_rpc::collect_fix_suggestions(actions, &uri, &text, &range,
            E::is_fix_kind);
        let p = self.pending_fix.take().expect("slot present — matched just above");
        vec![Action::Emit(Msg::DiagFixesReady { token: p.token, buffer_id: p.buffer_id,
            version: p.version, source: E::SOURCE, range: p.range, suggestions })]
    }
}

/// What `evaluate_fix_send` decided (E11 §3.3): hold for a later attempt site, resolve the slot
/// empty, or send this fully materialized request.
enum FixAttempt {
    Hold,
    ResolveEmpty,
    Send { uri: String, generation: u64, wire_range: Value, context: Vec<Value> },
}

/// An LSP position value → `(line, utf16-character)`.
fn pos(v: &Value) -> Option<(u32, u32)> {
    Some((v.get("line")?.as_u64()? as u32, v.get("character")?.as_u64()? as u32))
}

/// One raw LSP diagnostic's range as a byte range into `text`. `None` when the range is absent
/// or unmappable — the same rule `convert_diagnostics` drops on, so a converted diagnostic's
/// range and its raw's always agree.
fn raw_byte_range(d: &Value, text: &str) -> Option<std::ops::Range<usize>> {
    let r = d.get("range")?;
    let (s, e) = (r.get("start").and_then(pos)?, r.get("end").and_then(pos)?);
    crate::lsp_rpc::lsp_range_to_bytes(text, s, e)
}

/// One raw LSP diagnostic's `code` in the exact form `convert_diagnostics` preserves (a string
/// verbatim; any other JSON value stringified; absent → `None`).
fn raw_code(d: &Value) -> Option<String> {
    match d.get("code") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(other) => Some(other.to_string()),
        None => None,
    }
}

/// One raw LSP diagnostic's `message`, in the form `convert_diagnostics` preserves.
fn raw_message(d: &Value) -> &str { d.get("message").and_then(|m| m.as_str()).unwrap_or("") }

/// The deterministic TRIPLE match (E11 §3.3): a raw diagnostic identifies a request's anchor iff
/// its converted byte range, its `code`, AND its `message` all agree. Range alone is ambiguous
/// (overlapping/identical anchors exist); `code` and `message` ride the request because
/// `convert_diagnostics` preserves both verbatim, so the triple picks out the raw object exactly.
/// Fully identical raws are mutually indistinguishable and any one serves.
fn raw_matches(d: &Value, text: &str, range: &std::ops::Range<usize>,
    code: &Option<String>, message: &str) -> bool {
    raw_byte_range(d, text).as_ref() == Some(range)
        && raw_code(d) == *code
        && raw_message(d) == message
}

// ── The imperative shell: app-side handle + client thread + FlushGuard ──────────────────────────

/// Availability mirror shared between the app-side handle (`availability()` reads) and the client
/// thread (`SetAvailability` writes).
#[derive(Debug)]
struct Shared { availability: Mutex<Availability> }

/// The app-side `DiagnosticsProvider` handle. Cheap to construct (channel + `Shared`, **no thread,
/// no process**); `ensure_running` lazily spawns the client thread on first use.
// `LspProvider` stays `pub` (matches the pre-T1 `pub struct HarperLs`); `LspEngine` stays
// `pub(crate)` (engine identity/protocol is crate-internal). Deliberate sealed-trait-style gap,
// not a leak — `wordcartel` is a binary crate with no external consumers of these items.
#[derive(Debug)]
#[allow(private_bounds)]
pub struct LspProvider<E: LspEngine> {
    cmd_tx: Sender<Inbound>,
    pub(crate) rx: Option<Receiver<Inbound>>, // moved into the thread on first ensure_running
    shared: Arc<Shared>,
    started: bool,
    msg_tx: Sender<Msg>,
    cfg: ProviderConfig,
    _engine: std::marker::PhantomData<E>,
}

#[allow(private_bounds)] // see the struct-level rationale above
impl<E: LspEngine> LspProvider<E> {
    /// Construct the handle. Creates the `Inbound` channel + `Shared`; spawns nothing (idle is free).
    pub fn new(msg_tx: Sender<Msg>, cfg: ProviderConfig) -> Self {
        let (cmd_tx, rx) = std::sync::mpsc::channel();
        LspProvider {
            cmd_tx, rx: Some(rx),
            shared: Arc::new(Shared { availability: Mutex::new(Availability::Idle) }),
            started: false, msg_tx, cfg, _engine: std::marker::PhantomData,
        }
    }

    fn set_availability(&self, a: Availability) {
        *self.shared.availability.lock().expect("availability mutex") = a;
    }
}

impl<E: LspEngine> DiagnosticsProvider for LspProvider<E> {
    fn source(&self) -> DiagSource { E::SOURCE }
    fn install_hint(&self) -> &'static str { E::INSTALL_HINT }

    fn availability(&self) -> Availability {
        *self.shared.availability.lock().expect("availability mutex")
    }

    /// Spawn the client thread on first call. Latches `started` ONLY on a successful spawn — a spawn
    /// `Err` sets `Unavailable` and leaves `started` false (round-3 spawn-failure coverage, §3.1).
    fn ensure_running(&mut self) {
        if self.started { return; }
        let rx = match self.rx.take() { Some(r) => r, None => return };
        let msg_tx = self.msg_tx.clone();
        let inbound_tx = self.cmd_tx.clone();
        let shared = Arc::clone(&self.shared);
        let cfg = self.cfg.clone();
        let spawned = std::thread::Builder::new()
            .name(E::CLIENT_THREAD.into())
            .spawn(move || run_client::<E>(msg_tx, rx, inbound_tx, shared, cfg));
        match spawned {
            Ok(_) => self.started = true,
            Err(_) => self.set_availability(Availability::Unavailable),
        }
    }

    fn configure(&mut self, cfg: ProviderConfig) {
        self.cfg = cfg.clone();
        let _ = self.cmd_tx.send(Inbound::Cmd(Cmd::Configure(cfg)));
    }

    /// Forward a full-document sync. `Accepted::Yes` iff the send reached a live thread. An over-cap
    /// document is skipped (`Accepted::No`, no latch); a disconnected send flips availability.
    fn notify_change(&mut self, buffer_id: BufferId, version: u64,
        path: Option<std::path::PathBuf>, text: String) -> Accepted {
        if text.len() as u64 > DIAG_MAX_SEND_BYTES { return Accepted::No; }
        match self.cmd_tx.send(Inbound::Cmd(Cmd::Change { buffer_id, version, path, text })) {
            Ok(()) => Accepted::Yes,
            Err(_) => { self.set_availability(Availability::Unavailable); Accepted::No }
        }
    }

    fn notify_close(&mut self, buffer_id: BufferId) {
        let _ = self.cmd_tx.send(Inbound::Cmd(Cmd::Close { buffer_id }));
    }

    /// Forward an on-demand fix request (E11 §3.2). `Accepted::Yes` PROMISES exactly one
    /// `Msg::DiagFixesReady` for `token` — the client thread owes it from the machine, the
    /// deadline, or its `FlushGuard`. Two ways that promise could be broken, both refused here:
    /// before `ensure_running` the receiver is still ours, so a send would "succeed" into a
    /// channel nobody drains with no `FlushGuard` in existence (the overlay would strand in
    /// "fetching…" forever); after the thread dies the send errs, and — exactly as
    /// `notify_change` does — that flips availability. Non-blocking either way (hot-path law).
    fn request_fixes(&mut self, token: u64, buffer_id: BufferId, version: u64,
        range: std::ops::Range<usize>, code: Option<String>, message: String) -> Accepted {
        if !self.started { return Accepted::No; } // no thread ⟹ no terminal ⟹ no acceptance
        match self.cmd_tx.send(Inbound::Cmd(Cmd::RequestFixes { token, buffer_id, version,
            range, code, message })) {
            Ok(()) => Accepted::Yes,
            Err(_) => { self.set_availability(Availability::Unavailable); Accepted::No }
        }
    }

    fn reload_dictionary(&mut self) { let _ = self.cmd_tx.send(Inbound::Cmd(Cmd::ReloadDict)); }

    fn shutdown(&mut self) { let _ = self.cmd_tx.send(Inbound::Cmd(Cmd::Shutdown)); }

    fn suspend(&mut self) {
        if E::SUSPENDABLE && self.started { let _ = self.cmd_tx.send(Inbound::Cmd(Cmd::Suspend)); }
    }
}

/// Owns `cmd_rx` and runs the two-part flush on `Drop` — the last leg of the latch invariant
/// (§3.2). On ANY thread exit (clean, degrade, or panic-unwind), it emits an empty version-tagged
/// terminal for (1) every entry the pump recorded (`state.flush_outstanding()`) and (2) every
/// `Cmd::Change` still unread in the channel (the accepted-but-unrecorded gap).
pub(crate) struct FlushGuard<E: LspEngine> {
    pub(crate) state: ClientState<E>,
    pub(crate) cmd_rx: Receiver<Inbound>,
    pub(crate) msg_tx: Sender<Msg>,
}

impl<E: LspEngine> Drop for FlushGuard<E> {
    fn drop(&mut self) {
        for a in self.state.flush_outstanding() {
            if let Action::Emit(m) = a { let _ = self.msg_tx.send(m); }
        }
        while let Ok(inb) = self.cmd_rx.try_recv() {
            match inb {
                Inbound::Cmd(Cmd::Change { buffer_id, version, .. }) => {
                    let _ = self.msg_tx.send(Msg::DiagnosticsDone { buffer_id, version,
                        source: E::SOURCE, diagnostics: Vec::new() });
                }
                // E11 §3.2: an ACCEPTED-but-unread fix request owes its terminal too — thread
                // death between accept and apply must not strand a Fetching overlay.
                Inbound::Cmd(Cmd::RequestFixes { token, buffer_id, version, range, .. }) => {
                    let _ = self.msg_tx.send(Msg::DiagFixesReady { token, buffer_id, version,
                        source: E::SOURCE, range, suggestions: Vec::new() });
                }
                _ => {}
            }
        }
    }
}

/// The client thread entry point. Wraps the pump in `catch_unwind` so a panic cannot bypass the
/// `FlushGuard` (which lives in this outer scope and drops after the catch → flush always runs).
fn run_client<E: LspEngine>(msg_tx: Sender<Msg>, cmd_rx: Receiver<Inbound>, inbound_tx: Sender<Inbound>,
    shared: Arc<Shared>, cfg: ProviderConfig) {
    let mut guard = FlushGuard { state: ClientState::<E>::new(cfg), cmd_rx, msg_tx };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pump(&mut guard, &inbound_tx, &shared);
    }));
    // guard drops here → the two-part flush, even on a panic-unwind path.
}

/// Spawn the child + its reader thread; hand back the child and its stdin.
fn spawn_session<E: LspEngine>(inbound_tx: &Sender<Inbound>) -> std::io::Result<(Child, ChildStdin)> {
    let mut child = E::spawn_command()
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    spawn_reader::<E>(stdout, inbound_tx.clone());
    Ok((child, stdin))
}

/// The reader thread: loop `read_frame`, forward each frame as `Inbound::Server`; on read error /
/// EOF forward `Inbound::ServerEof` and exit (death is a message, never a hang — mirrors the M4
/// input-thread shape).
fn spawn_reader<E: LspEngine>(stdout: ChildStdout, inbound_tx: Sender<Inbound>) {
    let _ = std::thread::Builder::new().name(E::READER_THREAD.into()).spawn(move || {
        let mut r = BufReader::new(stdout);
        loop {
            match crate::lsp_rpc::read_frame(&mut r) {
                Ok(Some(v)) => { if inbound_tx.send(Inbound::Server(v)).is_err() { break; } }
                Ok(None) | Err(_) => { let _ = inbound_tx.send(Inbound::ServerEof); break; }
            }
        }
    });
}

/// What the pump does after running one batch of actions.
enum Control { Continue, Exit, Respawn, Park, Unpark }

/// The pump: spawn the child, feed the handshake, then `recv_timeout(next deadline)` over the single
/// `Inbound` channel — feeding `ClientState` and executing the actions it returns. Blocks on `recv`
/// with nothing pending (idle is free).
fn pump<E: LspEngine>(guard: &mut FlushGuard<E>, inbound_tx: &Sender<Inbound>, shared: &Arc<Shared>) {
    let start = Instant::now();
    let now = |s: &Instant| s.elapsed().as_millis() as u64;
    let (child, stdin) = match spawn_session::<E>(inbound_tx) {
        Ok(s) => s,
        Err(_) => {
            // NotFound (or any initial spawn failure) IS the runtime PATH detection (§3.2).
            set_availability(shared, Availability::Unavailable);
            let _ = guard.msg_tx.send(Msg::DiagProviderEvent { source: E::SOURCE,
                event: ProviderEvent::Degraded(E::INSTALL_HINT.into()) });
            return;
        }
    };
    let mut session: Option<(Child, ChildStdin)> = Some((child, stdin));
    let acts = guard.state.on_spawned(now(&start));
    let _ = run_actions(acts, &mut session, &guard.msg_tx, shared);
    set_availability(shared, Availability::Starting);

    let mut shutdown_at: Option<u64> = None;
    loop {
        let deadline = merge_deadline(guard.state.next_deadline(), shutdown_at);
        let acts = match wait_inbound(&guard.cmd_rx, deadline, now(&start)) {
            Wait::Closed => break, // app dropped the handle — end the thread (guard flushes).
            Wait::Timeout => {
                if let Some(sd) = shutdown_at {
                    if now(&start) >= sd {
                        if let Some((_, stdin)) = &mut session {
                            let _ = write_frame_to(stdin, &exit_notification());
                        }
                        break;
                    }
                }
                guard.state.on_deadline(now(&start))
            }
            Wait::Got(inb) => guard.state.on_inbound(inb, now(&start)),
        };
        match run_actions(acts, &mut session, &guard.msg_tx, shared) {
            Control::Continue => {}
            Control::Exit => break,
            Control::Respawn => {
                if let Some((mut c, s)) = session.take() { drop(s); let _ = c.kill(); let _ = c.wait(); }
                match spawn_session::<E>(inbound_tx) {
                    Ok((c, s)) => {
                        session = Some((c, s));
                        let acts = guard.state.on_spawned(now(&start));
                        let _ = run_actions(acts, &mut session, &guard.msg_tx, shared);
                    }
                    Err(_) => { let _ = inbound_tx.send(Inbound::ServerEof); } // consume the next budget step
                }
            }
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
        if guard.state.is_shutting_down() && shutdown_at.is_none() {
            shutdown_at = Some(now(&start) + SHUTDOWN_GRACE_MS);
        }
    }
    if let Some((mut c, _s)) = session.take() { let _ = c.kill(); let _ = c.wait(); }
}

fn exit_notification() -> Value { json!({"jsonrpc":"2.0","method":"exit"}) }

fn set_availability(shared: &Arc<Shared>, a: Availability) {
    *shared.availability.lock().expect("availability mutex") = a;
}

fn write_frame_to(stdin: &mut ChildStdin, v: &Value) -> std::io::Result<()> {
    crate::lsp_rpc::write_frame(stdin, v)
}

/// Execute one batch of actions in order; return the first control-flow action
/// (Respawn/Exit/Park/Unpark) hit.
fn run_actions(acts: Vec<Action>, session: &mut Option<(Child, ChildStdin)>,
    msg_tx: &Sender<Msg>, shared: &Arc<Shared>) -> Control {
    for a in acts {
        match a {
            Action::Send(v) => match session {
                Some((_, stdin)) => { let _ = write_frame_to(stdin, &v); }
                None => debug_assert!(false,
                    "Action::Send while parked (spec §5 rules make this unreachable)"),
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

fn merge_deadline(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) { (Some(x), Some(y)) => Some(x.min(y)), (x, None) => x, (None, y) => y }
}

enum Wait { Got(Inbound), Timeout, Closed }

/// Block on `cmd_rx` until `deadline_ms` (or forever when `None`). Translates timeout/disconnect.
fn wait_inbound(rx: &Receiver<Inbound>, deadline_ms: Option<u64>, now_ms: u64) -> Wait {
    match deadline_ms {
        None => match rx.recv() { Ok(i) => Wait::Got(i), Err(_) => Wait::Closed },
        Some(d) => {
            let dur = Duration::from_millis(d.saturating_sub(now_ms));
            match rx.recv_timeout(dur) {
                Ok(i) => Wait::Got(i),
                Err(RecvTimeoutError::Timeout) => Wait::Timeout,
                Err(RecvTimeoutError::Disconnected) => Wait::Closed,
            }
        }
    }
}

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
        const SUSPENDABLE: bool = true;
        fn spawn_command() -> Command { Command::new("wcartel-no-such-test-engine") }
        fn initialize_params(_cfg: &ProviderConfig) -> Value {
            json!({"processId": Value::Null, "capabilities": {}})
        }
        fn settings_push(_cfg: &ProviderConfig) -> Option<Value> { None }
        fn answer_request(_method: &str, _req: &Value, _cfg: &ProviderConfig) -> Option<Value> { None }
        fn classify(_d: &Value) -> DiagnosticKind { DiagnosticKind::Grammar }
        fn is_fix_kind(kind: &str) -> bool { kind == "quickfix" }
    }

    fn cfg() -> ProviderConfig {
        ProviderConfig { grammar: true, dictionary: None, max_file_length: 10_000, language: None }
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

    /// THE RESUME RACE (pre-merge gate finding): an edit arrives right as idle-suspend kills
    /// the JVM — `Cmd::Change` lands on the FIFO channel BEFORE the killed child's reader
    /// delivers its `ServerEof`. The pump unparks and `on_spawned` moves the phase to
    /// `Initializing`, so a phase-only drain would route the STALE EOF to `on_server_gone`:
    /// budget consumed, the queued post-resume check flushed as a false-empty terminal, and
    /// the fresh child killed. The deliberate-kill EOF must drain ACROSS the resume
    /// transition — and a real crash EOF afterward must still consume the budget.
    #[test]
    fn stale_suspend_eof_after_resume_spawn_is_drained() {
        let mut st = running();
        st.on_inbound(Inbound::Cmd(Cmd::Suspend), 0);
        // Change BEFORE the old child's EOF (the race interleaving): queued + Unpark.
        let resume = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 9,
            path: None, text: "back".into() }), 10);
        assert!(resume.iter().any(|a| matches!(a, Action::Unpark)));
        // The pump spawns the fresh child: phase leaves Suspended.
        let spawn = st.on_spawned(20);
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        // NOW the old deliberately-killed child's EOF arrives — it must be DRAINED.
        let stale = st.on_inbound(Inbound::ServerEof, 30);
        assert!(stale.is_empty(), "stale suspend-EOF after the resume spawn is drained");
        assert_eq!(st.spawn_attempts, 1, "no budget consumed by the stale EOF");
        // The queued post-resume check survived: the handshake still replays it.
        let replay = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":{"capabilities":{}}})), 40);
        assert!(sends(&replay).iter().any(|v| v["method"] == "textDocument/didOpen"),
            "the queued change was NOT flushed by the stale EOF — it replays");
        // A REAL crash EOF after Running still routes to on_server_gone (the counter did
        // not leak): budget consumed, respawn requested.
        let crash = st.on_inbound(Inbound::ServerEof, 50);
        assert!(crash.iter().any(|a| matches!(a, Action::Respawn)), "real crash still respawns");
        assert_eq!(st.spawn_attempts, 2, "real crash consumes the budget");
    }

    // ── T4: the on-demand `pending_fix` slot (E11 §3.3/§3.4) ────────────────────────────────

    fn fix_readies(acts: &[Action]) -> Vec<(u64, Vec<wordcartel_core::diagnostics::Suggestion>)> {
        acts.iter().filter_map(|a| match a {
            Action::Emit(Msg::DiagFixesReady { token, suggestions, .. }) =>
                Some((*token, suggestions.clone())),
            _ => None,
        }).collect()
    }

    fn publish_one(st: &mut ClientState<TestEngine>, uri: &str, at: u64) -> Vec<Action> {
        st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":uri,"diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                 "message":"m","code":"C1"}]}})), at)
    }

    /// `publish_one` with a caller-chosen `code`, so two publishes for the same document can be
    /// told apart by the triple-match identity a `RequestFixes` names.
    fn publish_code(st: &mut ClientState<TestEngine>, uri: &str, code: &str,
        at: u64) -> Vec<Action> {
        st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":uri,"diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                 "message":"m","code":code}]}})), at)
    }

    fn req(st: &mut ClientState<TestEngine>, token: u64, version: u64, at: u64) -> Vec<Action> {
        st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token, buffer_id: BufferId(0),
            version, range: 0..1, code: Some("C1".into()), message: "m".into() }), at)
    }

    #[test]
    fn fix_request_sends_when_running_open_and_raw_attributed() {
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5); // answers the await → last_raw tagged v1
        let out = req(&mut st, 7, 1, 10);
        let ca = sends(&out).into_iter().find(|v| v["method"] == "textDocument/codeAction")
            .expect("per-diagnostic codeAction sent");
        assert_eq!(ca["params"]["context"]["diagnostics"].as_array().unwrap().len(), 1,
            "the triple-matched RAW diagnostic is echoed verbatim (T1 outcome A shape)");
    }

    #[test]
    fn fix_response_attaches_all_and_clears_slot() {
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5);
        let out = req(&mut st, 7, 1, 10);
        let id = sends(&out).iter().find(|v| v["method"] == "textDocument/codeAction")
            .and_then(|v| v["id"].as_u64()).unwrap();
        let resp = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,"result":[
            {"kind":"quickfix","edit":{"changes":{"untitled:wcartel-0-1":[
                {"newText":"X","range":{"start":{"line":0,"character":0},
                                        "end":{"line":0,"character":1}}}]}}},
            {"kind":"quickfix","edit":{"changes":{"untitled:wcartel-0-1":[
                {"newText":"Y","range":{"start":{"line":0,"character":0},
                                        "end":{"line":0,"character":1}}}]}}}]})), 20);
        assert_eq!(fix_readies(&resp), vec![(7, vec![
            wordcartel_core::diagnostics::Suggestion::ReplaceWith("X".into()),
            wordcartel_core::diagnostics::Suggestion::ReplaceWith("Y".into())])],
            "attach-all through collect_fix_suggestions; token rides through");
        assert!(st.pending_fix.is_none());
    }

    #[test]
    fn stale_generation_fix_response_resolves_its_own_live_slot() {
        // T4-review Important: a stale-GENERATION response FOR the live slot must resolve it
        // NOW. The response consumed the request's `pending_requests` entry, so nothing can
        // ever answer that request again — waiting out the 10s leash would be a knowingly
        // futile "fetching…" (a no-silent-UI violation). The still-silent DISPLACED case is
        // pinned by `replacement_terminates_displaced_request_and_deregisters_its_id`.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5); // answers the await → last_raw tagged v1
        let out = req(&mut st, 7, 1, 10);
        let id = sends(&out).iter().find(|v| v["method"] == "textDocument/codeAction")
            .and_then(|v| v["id"].as_u64()).expect("the request went out under generation 1");
        // Bump the generation UNDER the in-flight request: a same-version didChange (so the
        // §3.3 change-invalidation never fires and the slot survives), then a watchdog
        // retirement, then the reopen — generation 2, the slot still owing token 7.
        change(&mut st, 0, 1, 20);
        st.on_deadline(20 + TestEngine::PUBLISH_TIMEOUT_MS); // retires generation 1
        change(&mut st, 0, 1, 1_030);                        // reopens under generation 2
        assert!(st.pending_fix.is_some(), "the slot is still live when the response lands");
        let late = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":[]})), 1_040);
        assert_eq!(fix_readies(&late), vec![(7, vec![])],
            "the stale-generation response resolves ITS OWN live slot immediately");
        assert!(st.pending_fix.is_none(), "the slot is cleared, not left owing its deadline");
        let expired = st.on_deadline(1_030 + FIX_REQUEST_TIMEOUT_MS);
        assert!(fix_readies(&expired).is_empty(),
            "and the leash emits nothing afterwards — exactly ONE terminal for token 7");
    }

    #[test]
    fn fix_deadline_fires_even_while_initializing() {
        // Round-1 finding-2 regression: the slot + deadline are live in EVERY phase.
        let mut st = ClientState::<TestEngine>::new(cfg());
        st.on_spawned(0); // Initializing
        let out = req(&mut st, 9, 1, 0);
        assert!(out.is_empty(), "slot materialized, nothing sent, nothing queued");
        assert_eq!(st.next_deadline(), Some(FIX_REQUEST_TIMEOUT_MS),
            "the 10s leash is visible to the pump during warm");
        let expired = st.on_deadline(FIX_REQUEST_TIMEOUT_MS);
        assert_eq!(fix_readies(&expired), vec![(9, vec![])], "honest empty terminal at 10s");
    }

    #[test]
    fn stale_raw_after_didchange_never_sends_and_change_invalidates() {
        // Round-2 finding-2 + round-3: one-snapshot condition + change-invalidation.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5); // last_raw tagged v1
        change(&mut st, 0, 2, 10); // didChange advances text/our_version to v2
        let out = req(&mut st, 3, 2, 15); // request against v2; raws are v1
        assert!(sends(&out).iter().all(|v| v["method"] != "textDocument/codeAction"),
            "no send against mismatched snapshots");
        // A further change PAST a pending slot resolves it empty immediately.
        let inv = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 3,
            path: None, text: "zz".into() }), 20);
        assert_eq!(fix_readies(&inv), vec![(3u64, vec![])],
            "change-invalidation emits the token terminal at once");
    }

    #[test]
    fn replacement_terminates_displaced_request_and_deregisters_its_id() {
        // Round-2 finding-3: exactly-once holds for the DISPLACED request too.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5);
        let a = req(&mut st, 1, 1, 10);
        let id_a = sends(&a).iter().find(|v| v["method"] == "textDocument/codeAction")
            .and_then(|v| v["id"].as_u64()).unwrap();
        let b = req(&mut st, 2, 1, 11);
        assert_eq!(fix_readies(&b), vec![(1, vec![])], "displaced token 1 terminated at replacement");
        assert!(!st.pending_requests.contains_key(&id_a),
            "the displaced request's id is DE-REGISTERED — otherwise every unanswered fix \
             request leaks a pending_requests entry until the next respawn clears the map");
        let late = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id_a,
            "result":[]})), 12);
        assert!(fix_readies(&late).is_empty(), "late response to the displaced id → unknown-id arm");
    }

    #[test]
    fn a_request_for_a_stale_version_holds_even_when_its_raws_are_tagged() {
        // The OTHER half of §3.3's one-snapshot condition, which the version-mismatch test above
        // cannot reach: raws tagged v1 AND a request naming v1 is still not enough — the DOCUMENT
        // must be at v1 too. A slot minted after the buffer already advanced (no `on_change`
        // follows, so change-invalidation never runs) would otherwise send v1's raws matched
        // against v2's text, under v2's uri — three artifacts, two snapshots.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5); // last_raw tagged v1
        change(&mut st, 0, 2, 10);                       // the doc moves to v2; the raws stay v1
        let out = req(&mut st, 12, 1, 15);               // …and NOW a request against the stale v1
        assert!(sends(&out).iter().all(|v| v["method"] != "textDocument/codeAction"),
            "the tag matches the request, but the document has moved past both");
        assert!(fix_readies(&out).is_empty(), "held for its leash — not a no-triple-match resolve");
        assert_eq!(st.pending_fix.as_ref().map(|p| p.deadline), Some(15 + FIX_REQUEST_TIMEOUT_MS),
            "and the held slot still owes exactly one terminal, at its own deadline");
    }

    #[test]
    fn timeout_retirement_cycles_do_not_leak_and_late_publishes_drop() {
        // Round-3 Critical-1 + round-4 Important-2, pinned over N CYCLES with per-cycle
        // mapping/count assertions (plan-gate finding 5: one cycle without uri_owner
        // assertions cannot catch the leak).
        let mut st = running();
        let mut base = 0u64;
        for cycle in 1..=3u64 {
            let version = cycle;
            st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version,
                path: None, text: "x".into() }), base);
            let expired = st.on_deadline(base + TestEngine::FIRST_CHECK_TIMEOUT_MS.unwrap());
            assert!(!diag_dones(&expired).is_empty(), "cycle {cycle}: check terminal emitted");
            let closes = sends(&expired).iter()
                .filter(|v| v["method"] == "textDocument/didClose").count();
            assert_eq!(closes, 1, "cycle {cycle}: exactly one didClose per retirement");
            assert!(st.uri_owner.is_empty(),
                "cycle {cycle}: the retired mapping is REMOVED — uri_owner holds only live URIs");
            let late = publish_one(&mut st,
                &format!("untitled:wcartel-0-{cycle}"), base + 999);
            assert!(late.is_empty(), "cycle {cycle}: late publish to the retired uri drops whole");
            base += 1_000_000;
        }
        // The next check reopens under generation 4 (three retirements consumed 1..=3).
        let reopen = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 9,
            path: None, text: "b".into() }), base);
        assert!(sends(&reopen).iter().any(|v| v["method"] == "textDocument/didOpen"
            && v["params"]["textDocument"]["uri"] == "untitled:wcartel-0-4"));
        assert_eq!(st.uri_owner.len(), 1, "exactly the one live URI after N cycles");
        // T4-review fold-in: the THIRD reopen path — suspend → unpark resume — deliberately
        // bypasses `on_server_gone` (the expected-EOF drain), so `on_spawned` is what must
        // retire the pre-suspend URI. Without its clear, every idle cycle strands one entry.
        for cycle in 1..=2u64 {
            st.on_inbound(Inbound::Cmd(Cmd::Suspend), base);
            st.on_inbound(Inbound::ServerEof, base); // the deliberate-kill EOF drains (E10)
            let resume = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0),
                version: 9 + cycle, path: None, text: "c".into() }), base);
            assert!(resume.iter().any(|a| matches!(a, Action::Unpark)),
                "resume cycle {cycle}: a Change while parked warrants a fresh child");
            let spawn = st.on_spawned(base);
            let id = sends(&spawn)[0]["id"].as_u64().unwrap();
            let replay = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
                "result":{"capabilities":{}}})), base);
            let uri = sends(&replay).iter().find(|v| v["method"] == "textDocument/didOpen")
                .and_then(|v| v["params"]["textDocument"]["uri"].as_str())
                .expect("the replayed change reopens the document").to_string();
            assert_eq!(st.uri_owner.len(), 1,
                "resume cycle {cycle}: no-leak is UNIFORM across all three reopen paths");
            assert!(st.uri_owner.contains_key(&uri), "resume cycle {cycle}: and it is the LIVE uri");
            base += 1_000_000;
        }
    }

    #[test]
    fn retirement_then_buffer_close_sends_exactly_one_didclose() {
        // Round-5 Important-2: the wire frame is gated on d.open.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        let retired = st.on_deadline(TestEngine::FIRST_CHECK_TIMEOUT_MS.unwrap());
        let n1 = sends(&retired).iter().filter(|v| v["method"] == "textDocument/didClose").count();
        let closed = st.on_inbound(Inbound::Cmd(Cmd::Close { buffer_id: BufferId(0) }), 10);
        let n2 = sends(&closed).iter().filter(|v| v["method"] == "textDocument/didClose").count();
        assert_eq!(n1 + n2, 1, "exactly ONE didClose across retirement + close");
    }

    #[test]
    fn close_resolves_pending_fix_and_a_late_response_is_silent() {
        // Round-3 Important-2: the document-close leg of exactly-once.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5);
        let out = req(&mut st, 4, 1, 10);
        let id = sends(&out).iter().find(|v| v["method"] == "textDocument/codeAction")
            .and_then(|v| v["id"].as_u64()).unwrap();
        let closed = st.on_inbound(Inbound::Cmd(Cmd::Close { buffer_id: BufferId(0) }), 20);
        assert!(fix_readies(&closed).contains(&(4, vec![])), "close emits the token terminal");
        let late = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,"result":[]})), 30);
        assert!(fix_readies(&late).is_empty(), "de-registered id → silence, no double terminal");
    }

    #[test]
    fn suspend_resume_holds_the_slot_and_sends_after_replay_and_fresh_publish() {
        // Spec §3.5's full ordering: suspend → change → unpark → slot persists → replay →
        // publish re-tags → send. (TestEngine::SUSPENDABLE = true.)
        let mut st = running();
        st.on_inbound(Inbound::Cmd(Cmd::Suspend), 0);
        st.on_inbound(Inbound::ServerEof, 1); // expected-EOF drain (E10)
        let resume = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1,
            path: None, text: "x".into() }), 2);
        assert!(resume.iter().any(|a| matches!(a, Action::Unpark)));
        let held = req(&mut st, 6, 1, 3);
        assert!(held.is_empty(), "slot held — no send, no emissions yet");
        assert!(st.queued.iter().all(|c| !matches!(c, Cmd::RequestFixes { .. })),
            "RequestFixes is NEVER in `queued` — the slot IS its queue (finding-2 class)");
        let spawn = st.on_spawned(4);
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        let replay = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":{"capabilities":{}}})), 5);
        // The publish that ANSWERS the replayed didOpen re-tags last_raw → the slot sends.
        // Take the URI from that didOpen: the reopen mints whatever generation is next, and
        // this interleaving never opened the doc before the suspend, so it is generation 1 —
        // the plan text's literal "-0-2" presumed a pre-suspend open this ordering omits.
        let uri = sends(&replay).iter().find(|v| v["method"] == "textDocument/didOpen")
            .and_then(|v| v["params"]["textDocument"]["uri"].as_str())
            .expect("the replayed change reopens the document").to_string();
        let pubd = publish_one(&mut st, &uri, 6);
        assert!(sends(&pubd).iter().any(|v| v["method"] == "textDocument/codeAction"),
            "attempt site (c): send fires after the awaited publish re-tags last_raw");
    }

    #[test]
    fn every_attributed_publish_updates_last_raw_in_lockstep_with_the_store() {
        // E11 T10 — the INVERSE of the retired §3.3 await-attribution rule (this test formerly
        // read `unsolicited_publish_does_not_update_last_raw`). Pinned with DISTINGUISHABLE
        // arrays: the first publish answers the await and carries code C1; the second (NO await
        // live — ltex's config-triggered republish shape) carries ONLY C9. That republish is
        // attributed, so it updates the diagnostics STORE — the writer now sees C9 and nothing
        // else. `last_raw` must follow it: C9 becomes the fixable identity and C1 stops being
        // one. Retaining C1's raws instead is exactly the desync that made a visible diagnostic
        // unfixable in the live probe.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5); // answers the await: code C1
        let republish = publish_code(&mut st, "untitled:wcartel-0-1", "C9", 6); // NO await live
        assert_eq!(diag_dones(&republish), vec![(BufferId(0), 1)],
            "the republish DOES update the store — which is why it must update last_raw too");
        let c1 = req(&mut st, 8, 1, 10);
        assert_eq!(fix_readies(&c1), vec![(8, vec![])],
            "C1 left the store, so it is no longer fixable — no stale echo survives it");
        let c9 = st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 9,
            buffer_id: BufferId(0), version: 1, range: 0..1,
            code: Some("C9".into()), message: "m".into() }), 11);
        assert!(sends(&c9).iter().any(|v| v["method"] == "textDocument/codeAction"),
            "C9 — what the writer actually sees — triple-matches and reaches the wire");
    }

    #[test]
    fn straggler_publish_between_checks_does_not_desync_anchor_from_echo() {
        // THE live-probe F1 regression: apply an ltex quick-fix, undo it, and that diagnostic
        // could never be fixed again. ltex emits more than one publish per didChange, so a
        // straggler from the PREVIOUS check lands inside the NEXT check's await window. Under
        // await-attribution the store took one publish and `last_raw` took the other, both
        // tagged the same version and naming different content — a permanent desync that killed
        // fixes for precisely the diagnostic under repair. Lockstep attribution makes the pairing
        // structural, so no publish interleaving can separate them.
        let mut st = running();
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1,
            path: None, text: "ab".into() }), 0);
        publish_code(&mut st, "untitled:wcartel-0-1", "OLD", 5); // answers await(v1)
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 2,
            path: None, text: "cd".into() }), 10);               // await(v2) now live
        // The straggler — the PREVIOUS check's content, arriving inside v2's await window and so
        // ANSWERING await(v2). This is the publish the retired rule tagged v2's raws from.
        publish_code(&mut st, "untitled:wcartel-0-1", "OLD", 11);
        // …and now v2's real publish, with no await left for it to answer.
        let real = publish_code(&mut st, "untitled:wcartel-0-1", "NEW", 12);
        assert_eq!(diag_dones(&real), vec![(BufferId(0), 2)],
            "the store shows v2's content at v2 — this is what the writer sees underlined");
        let out = st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 21,
            buffer_id: BufferId(0), version: 2, range: 0..1,
            code: Some("NEW".into()), message: "m".into() }), 15);
        assert!(sends(&out).iter().any(|v| v["method"] == "textDocument/codeAction"),
            "the diagnostic the writer can SEE is fixable — the echo tracks the store");
        assert!(fix_readies(&out).is_empty(),
            "…and it is NOT resolved empty, which is what the desync produced live");
    }

    #[test]
    fn no_triple_match_resolves_empty_immediately() {
        // §3.3: version-matched raws present but nothing matches the request identity.
        let mut st = running();
        change(&mut st, 0, 1, 0);
        publish_one(&mut st, "untitled:wcartel-0-1", 5); // raws hold C1 only
        let out = st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 4,
            buffer_id: BufferId(0), version: 1, range: 0..1,
            code: Some("NOPE".into()), message: "m".into() }), 10);
        assert_eq!(fix_readies(&out), vec![(4, vec![])], "no-match leg emits the empty terminal");
        assert!(sends(&out).iter().all(|v| v["method"] != "textDocument/codeAction"));
    }

    #[test]
    fn server_gone_flushes_a_pending_fix() {
        // §3.4 exactly-once, server-gone leg: the slot joins flush_outstanding's coverage.
        let mut st = running();
        st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 5, buffer_id: BufferId(0),
            version: 1, range: 0..1, code: None, message: "m".into() }), 0); // held (no raws)
        let gone = st.on_inbound(Inbound::ServerEof, 1);
        assert!(fix_readies(&gone).contains(&(5, vec![])),
            "on_server_gone's flush emits the held slot's token terminal");
        assert!(st.pending_fix.is_none());
    }

    #[test]
    fn flush_guard_drains_unread_request_fixes_from_the_channel() {
        // §3.2 FlushGuard extension — mirror the shipped flush_guard_drop channel pattern.
        let (msg_tx, msg_rx) = std::sync::mpsc::channel::<Msg>();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Inbound>();
        let state = ClientState::<TestEngine>::new(cfg());
        cmd_tx.send(Inbound::Cmd(Cmd::RequestFixes { token: 6, buffer_id: BufferId(0),
            version: 1, range: 0..1, code: None, message: "m".into() })).unwrap();
        drop(FlushGuard { state, cmd_rx, msg_tx });
        let got: Vec<u64> = std::iter::from_fn(|| msg_rx.try_recv().ok())
            .filter_map(|m| if let Msg::DiagFixesReady { token, suggestions, .. } = m {
                assert!(suggestions.is_empty()); Some(token) } else { None }).collect();
        assert_eq!(got, vec![6], "an UNREAD RequestFixes still gets its empty terminal on drop");
    }

    #[test]
    fn shutdown_with_a_live_slot_terminates_it_exactly_once_at_guard_drop() {
        // §3.4 exactly-once, the SHUTDOWN leg: `Cmd::Shutdown` does not flush — the terminal
        // arrives from `FlushGuard::drop` after `Control::Exit`. That is an interaction
        // between two code paths, so pin it: exactly one terminal for the token across the
        // whole sequence, and the flush stays idempotent (a prior `on_server_gone` flush must
        // not let the guard's own emit a second one).
        let (msg_tx, msg_rx) = std::sync::mpsc::channel::<Msg>();
        let (_cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Inbound>();
        let mut state = running();
        let held = state.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 11,
            buffer_id: BufferId(0), version: 1, range: 0..1, code: None,
            message: "m".into() }), 0); // held (no doc, no raws) — it survives to the drop
        assert!(fix_readies(&held).is_empty(), "accepted, nothing owed yet");
        let sd = state.on_inbound(Inbound::Cmd(Cmd::Shutdown), 1);
        assert!(fix_readies(&sd).is_empty(), "Cmd::Shutdown does NOT flush");
        assert!(state.pending_fix.is_some(), "the slot survives the shutdown request");
        let id = sends(&sd)[0]["id"].as_u64().expect("the shutdown request id");
        let exit = state.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":Value::Null})), 2);
        assert!(exit.iter().any(|a| matches!(a, Action::Exit)), "the response drives Exit");
        assert!(fix_readies(&exit).is_empty(), "still nothing emitted at Exit");
        drop(FlushGuard { state, cmd_rx, msg_tx });
        let got: Vec<u64> = std::iter::from_fn(|| msg_rx.try_recv().ok())
            .filter_map(|m| if let Msg::DiagFixesReady { token, suggestions, .. } = m {
                assert!(suggestions.is_empty()); Some(token) } else { None }).collect();
        assert_eq!(got, vec![11], "EXACTLY one terminal, emitted by the guard's drop");
        // The idempotence half, on an identical sequence: the guard's drop calls
        // `flush_outstanding`, so a second call must yield nothing (no double terminal).
        let mut st2 = running();
        st2.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 12, buffer_id: BufferId(0),
            version: 1, range: 0..1, code: None, message: "m".into() }), 0);
        st2.on_inbound(Inbound::Cmd(Cmd::Shutdown), 1);
        assert_eq!(fix_readies(&st2.flush_outstanding()), vec![(12, vec![])]);
        assert!(fix_readies(&st2.flush_outstanding()).is_empty(), "a second flush yields nothing");
    }

    /// The Outcome-B seam engine: `TestEngine`'s impl verbatim with the single const flipped,
    /// so BOTH `context.diagnostics` shapes stay under test whatever T1 ruled for ltex.
    #[derive(Debug)]
    struct TestEngineAllRaws;
    impl LspEngine for TestEngineAllRaws {
        const SOURCE: DiagSource = DiagSource::Plugin("test-engine");
        const INSTALL_HINT: &'static str = "test engine unavailable";
        const CRASHED_HINT: &'static str = "test engine crashed";
        const LANGUAGE_ID: &'static str = "markdown";
        const CLIENT_THREAD: &'static str = "wcartel-test-client";
        const READER_THREAD: &'static str = "wcartel-test-read";
        const PUBLISH_TIMEOUT_MS: u64 = 1_000;
        const FIRST_CHECK_TIMEOUT_MS: Option<u64> = Some(30_000);
        const SUSPENDABLE: bool = true;
        const FIX_CONTEXT_ALL_RAWS: bool = true;
        fn spawn_command() -> Command { Command::new("wcartel-no-such-test-engine") }
        fn initialize_params(_cfg: &ProviderConfig) -> Value {
            json!({"processId": Value::Null, "capabilities": {}})
        }
        fn settings_push(_cfg: &ProviderConfig) -> Option<Value> { None }
        fn answer_request(_method: &str, _req: &Value, _cfg: &ProviderConfig) -> Option<Value> { None }
        fn classify(_d: &Value) -> DiagnosticKind { DiagnosticKind::Grammar }
        fn is_fix_kind(kind: &str) -> bool { kind == "quickfix" }
    }

    #[test]
    fn all_raws_engine_echoes_the_full_retained_array_with_the_matched_range() {
        let mut st = ClientState::<TestEngineAllRaws>::new(cfg());
        // running() equivalent inline (the helper is TestEngine-typed): spawn + init response.
        let spawn = st.on_spawned(0);
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":{"capabilities":{}}})), 0);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1,
            path: None, text: "ab".into() }), 0);
        st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-0-1","diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                 "message":"m","code":"C1"},
                {"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":2}},
                 "message":"n","code":"C2"}]}})), 5);
        let out = st.on_inbound(Inbound::Cmd(Cmd::RequestFixes { token: 7,
            buffer_id: BufferId(0), version: 1, range: 0..1,
            code: Some("C1".into()), message: "m".into() }), 10);
        let ca = sends(&out).into_iter().find(|v| v["method"] == "textDocument/codeAction").unwrap();
        assert_eq!(ca["params"]["context"]["diagnostics"].as_array().unwrap().len(), 2,
            "ALL retained raws echoed under the all-raws shape");
        assert_eq!(ca["params"]["range"], json!({"start":{"line":0,"character":0},
            "end":{"line":0,"character":1}}), "range stays the MATCHED raw's verbatim range");
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

    // ── T5: the app-side handle's fix request (E11 §3.2 — Accepted ⟹ a terminal is owed) ────

    /// A never-spawned handle STILL HOLDS the receiver in `rx`, so `cmd_tx.send` returns `Ok`
    /// into a channel NOBODY drains — and with no client thread there is no `FlushGuard` to owe
    /// the terminal either. An `Accepted::Yes` there would strand the overlay in "fetching…"
    /// forever, so the un-started handle must REFUSE.
    #[test]
    fn request_fixes_without_a_client_thread_is_not_accepted() {
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel::<Msg>();
        let mut p = LspProvider::<TestEngine>::new(msg_tx, cfg());
        assert!(p.rx.is_some(), "precondition: the receiver is still app-side — no thread runs");
        assert_eq!(p.request_fixes(1, BufferId(0), 1, 0..1, None, "m".into()), Accepted::No,
            "no client thread ⟹ no terminal is possible ⟹ never accept");
    }

    /// The disconnected-channel leg, mirroring `notify_change`: a started handle whose thread has
    /// died flips availability and refuses (the thread's `FlushGuard` already ran, so no terminal
    /// can come from a send that lands nowhere).
    #[test]
    fn request_fixes_on_a_dead_channel_marks_unavailable_and_is_not_accepted() {
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel::<Msg>();
        let mut p = LspProvider::<TestEngine>::new(msg_tx, cfg());
        p.started = true; // pretend ensure_running spawned the thread…
        p.rx = None;      // …and that the thread then exited, dropping the receiver.
        assert_eq!(p.request_fixes(2, BufferId(0), 1, 0..1, None, "m".into()), Accepted::No);
        assert_eq!(p.availability(), Availability::Unavailable);
    }

    /// The accepting leg: with the client up, the request goes out VERBATIM — the token and the
    /// `code`/`message` disambiguators must survive the hop, or §3.3 cannot pick the raw to echo.
    #[test]
    fn request_fixes_sends_the_command_verbatim_when_the_client_is_up() {
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel::<Msg>();
        let mut p = LspProvider::<TestEngine>::new(msg_tx, cfg());
        p.started = true; // the thread is "up"; its receiver is still `p.rx`, so the test can read it
        assert_eq!(p.request_fixes(9, BufferId(3), 7, 2..5, Some("C1".into()), "m".into()),
            Accepted::Yes);
        let rx = p.rx.take().expect("receiver");
        match rx.try_recv() {
            Ok(Inbound::Cmd(Cmd::RequestFixes { token, buffer_id, version, range, code, message })) => {
                assert_eq!((token, buffer_id, version, range), (9, BufferId(3), 7, 2..5));
                assert_eq!(code.as_deref(), Some("C1"));
                assert_eq!(message, "m");
            }
            _ => panic!("expected exactly one Cmd::RequestFixes on the wire"),
        }
    }
}
