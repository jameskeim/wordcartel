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

/// Grace after `shutdown` before the pump forces `exit` + kills the child (bounded quit latency).
const SHUTDOWN_GRACE_MS: u64 = 1_000;
/// Respawn budget per session — the initial spawn counts as the first (spec §3.4; anti-crash-loop).
const MAX_SPAWN_ATTEMPTS: u32 = 3;

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
}
/// A didOpen/didChange awaiting its `publishDiagnostics` (or the publish watchdog).
pub(crate) struct AwaitPublish { pub(crate) our_version: u64, pub(crate) generation: u64, pub(crate) deadline: u64 }
/// Converted diagnostics parked while a batched codeAction is in flight (or its watchdog).
pub(crate) struct Assembly {
    pub(crate) our_version: u64, pub(crate) generation: u64,
    pub(crate) diags: Vec<Diagnostic>, pub(crate) deadline: u64,
}
/// What an outstanding JSON-RPC request id means when its response lands.
pub(crate) enum PendingKind {
    Initialize, Shutdown, CodeAction { buffer_id: BufferId, generation: u64, our_version: u64 },
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
    pub(crate) assembling: HashMap<BufferId, Assembly>,
    pub(crate) spawn_attempts: u32,
    /// True once this child process has produced its first owned-URI publish (E10 §4) — gates
    /// which watchdog deadline `on_change` stamps. Reset in `on_spawned` so a respawned child
    /// re-enters the warm phase.
    pub(crate) first_publish_seen: bool,
    pub(crate) _engine: std::marker::PhantomData<E>,
}

impl<E: LspEngine> ClientState<E> {
    /// A fresh machine, pre-handshake. `spawn_attempts` starts at 1 — the initial spawn counts.
    pub(crate) fn new(cfg: ProviderConfig) -> Self {
        ClientState {
            phase: Phase::Initializing, cfg, docs: HashMap::new(), uri_owner: HashMap::new(),
            next_generation: 1, queued: Vec::new(), next_id: 1, pending_requests: HashMap::new(),
            awaiting_publish: HashMap::new(), assembling: HashMap::new(), spawn_attempts: 1,
            first_publish_seen: false,
            _engine: std::marker::PhantomData,
        }
    }

    fn alloc_id(&mut self) -> u64 { let id = self.next_id; self.next_id += 1; id }

    /// True once `Cmd::Shutdown` was applied — the pump arms its grace timer off this.
    pub(crate) fn is_shutting_down(&self) -> bool { self.phase == Phase::ShuttingDown }

    /// The soonest watchdog deadline, if any — the pump's `recv_timeout` bound (idle = `None`).
    pub(crate) fn next_deadline(&self) -> Option<u64> {
        self.awaiting_publish.values().map(|a| a.deadline)
            .chain(self.assembling.values().map(|a| a.deadline))
            .min()
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
        self.pending_requests.clear();
        let id = self.alloc_id();
        self.pending_requests.insert(id, PendingKind::Initialize);
        vec![Action::Send(self.initialize_request(id))]
    }

    /// The top-level router (spec §3.3).
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
            // EXPECTED — drained, never routed to the crash/respawn path.
            Inbound::ServerEof => if self.phase == Phase::Suspended { Vec::new() }
                else { self.on_server_gone(now) },
        }
    }

    fn apply_cmd(&mut self, c: Cmd, now: u64) -> Vec<Action> {
        match c {
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
        let mut out = Vec::new();
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
                DocState { uri, lsp_version, our_version: version, generation, text, open: true });
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
        let outstanding = self.awaiting_publish.remove(&buffer_id).map(|a| a.our_version)
            .or_else(|| self.assembling.remove(&buffer_id).map(|a| a.our_version));
        if let Some(version) = outstanding {
            out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id, version,
                source: E::SOURCE, diagnostics: Vec::new() }));
        }
        if let Some(d) = self.docs.remove(&buffer_id) {
            self.uri_owner.remove(&d.uri);
            out.push(Action::Send(json!({"jsonrpc":"2.0","method":"textDocument/didClose",
                "params":{"textDocument":{"uri":d.uri}}})));
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
            Some(PendingKind::CodeAction { buffer_id, generation, our_version }) =>
                self.on_codeaction_response(buffer_id, generation, our_version, &v),
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
        out
    }

    /// A `publishDiagnostics` notification. URI-keyed generation attribution (spec §3.3 Receive):
    /// an absent uri → drop; empty result → emit terminal + clear awaiting; non-empty → eager-
    /// assemble one batched codeAction, parking the converted set.
    fn on_publish(&mut self, v: &Value, now: u64) -> Vec<Action> {
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
        // The publish arrived; retire the await slot. Its generation must match the URI-attributed
        // one (both are stamped from the same reopen) — a soundness cross-check on the tag.
        if let Some(a) = self.awaiting_publish.remove(&buffer_id) {
            debug_assert_eq!(a.generation, generation, "awaiting generation matches attributed publish");
        }
        if converted.is_empty() {
            return vec![Action::Emit(Msg::DiagnosticsDone { buffer_id, version: tagged,
                source: E::SOURCE, diagnostics: Vec::new() })];
        }
        let (start, end) = match raw_envelope(&raw) {
            Some(e) => e,
            None => return vec![Action::Emit(Msg::DiagnosticsDone { buffer_id, version: tagged,
                source: E::SOURCE, diagnostics: converted })], // no envelope → emit converted suggestionless
        };
        let id = self.alloc_id();
        self.pending_requests.insert(id, PendingKind::CodeAction { buffer_id, generation,
            our_version: tagged });
        self.assembling.insert(buffer_id, Assembly { our_version: tagged, generation,
            diags: converted, deadline: now + E::CODEACTION_TIMEOUT_MS });
        vec![Action::Send(codeaction_request(id, &uri, start, end, &raw))]
    }

    /// A codeAction RESPONSE. Remove the assembly FIRST (terminal-guarantee), attach suggestions to
    /// the parked diagnostics, and emit. A superseded generation is discarded (never emitted against
    /// newer text) — an empty terminal still clears the latch.
    fn on_codeaction_response(&mut self, buffer_id: BufferId, generation: u64, our_version: u64,
        v: &Value) -> Vec<Action> {
        // Stale-response guard: consume the parked assembly ONLY when BOTH its generation AND its
        // our_version match this response's request. A request that stalled past its watchdog (v1)
        // could otherwise consume a NEWER assembly (v2, re-parked by a later publish under the same
        // generation) and attach v1-computed edits. On mismatch, DISCARD this response and leave the
        // assembly untouched — it still terminates via its own response or watchdog (no wedged latch).
        match self.assembling.get(&buffer_id) {
            Some(a) if a.our_version == our_version && a.generation == generation => {}
            _ => return Vec::new(),
        }
        let assembly = self.assembling.remove(&buffer_id).expect("assembly present — matched just above");
        let live = self.docs.get(&buffer_id)
            .map(|d| d.open && d.generation == generation && assembly.generation == generation)
            .unwrap_or(false);
        if !live {
            // Superseded mid-fetch: discard the (possibly wrong-range) fixes but clear the latch.
            return vec![Action::Emit(Msg::DiagnosticsDone { buffer_id,
                version: assembly.our_version, source: E::SOURCE, diagnostics: Vec::new() })];
        }
        let (uri, text) = self.docs.get(&buffer_id)
            .map(|d| (d.uri.clone(), d.text.clone())).unwrap_or_default();
        let actions = v["result"].as_array().cloned().unwrap_or_default();
        let mut diags = assembly.diags;
        for d in &mut diags {
            for a in &actions {
                if let Some(s) = crate::lsp_rpc::quickfix_suggestion(a, &uri, &text, &d.range) {
                    d.suggestions.push(s);
                    break;
                }
            }
        }
        diags.sort_by_key(|d| d.range.start);
        vec![Action::Emit(Msg::DiagnosticsDone { buffer_id, version: assembly.our_version,
            source: E::SOURCE, diagnostics: diags })]
    }

    /// Convert an LSP diagnostics array to our byte-ranged set against `text` (spec §6/§7). Drops
    /// unconvertible ranges and — when `!cfg.grammar` — Grammar-classified diagnostics.
    fn convert_diagnostics(&self, raw: &[Value], text: &str) -> Vec<Diagnostic> {
        let mut out = Vec::new();
        for d in raw {
            let r = match d.get("range") { Some(r) => r, None => continue };
            let (s, e) = match (r.get("start").and_then(pos), r.get("end").and_then(pos)) {
                (Some(s), Some(e)) => (s, e), _ => continue,
            };
            let range = match crate::lsp_rpc::lsp_range_to_bytes(text, s, e) {
                Some(r) => r, None => continue,
            };
            let kind = E::classify(d);
            if !self.cfg.grammar && kind == DiagnosticKind::Grammar { continue; }
            let message = d.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            let code = match d.get("code") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(other) => Some(other.to_string()),
                None => None,
            };
            let href = d.get("codeDescription").and_then(|c| c.get("href"))
                .and_then(|h| h.as_str()).map(str::to_string);
            out.push(Diagnostic { range, kind, source: E::SOURCE, code, href, message,
                suggestions: Vec::new() });
        }
        out.sort_by_key(|d| d.range.start);
        out
    }

    /// Watchdogs (spec §3.4). Both remove the tracked entry BEFORE emitting (terminal-guarantee):
    /// publish past deadline → empty terminal; assembly past deadline → converted, suggestionless.
    pub(crate) fn on_deadline(&mut self, now: u64) -> Vec<Action> {
        let mut out = Vec::new();
        let expired_pub: Vec<BufferId> = self.awaiting_publish.iter()
            .filter(|(_, a)| now >= a.deadline).map(|(b, _)| *b).collect();
        for b in expired_pub {
            if let Some(a) = self.awaiting_publish.remove(&b) {
                out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id: b, version: a.our_version,
                    source: E::SOURCE, diagnostics: Vec::new() }));
            }
        }
        let expired_asm: Vec<BufferId> = self.assembling.iter()
            .filter(|(_, a)| now >= a.deadline).map(|(b, _)| *b).collect();
        for b in expired_asm {
            if let Some(a) = self.assembling.remove(&b) {
                out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id: b, version: a.our_version,
                    source: E::SOURCE, diagnostics: a.diags }));
            }
        }
        out
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
    /// `awaiting_publish` + `assembling` + queued `Cmd::Change`, removing each as it emits. Idempotent
    /// (a second call emits nothing) — the FlushGuard's drop can call it after `on_server_gone` did.
    pub(crate) fn flush_outstanding(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        for (b, a) in self.awaiting_publish.drain() {
            out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id: b, version: a.our_version,
                source: E::SOURCE, diagnostics: Vec::new() }));
        }
        for (b, a) in self.assembling.drain() {
            out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id: b, version: a.our_version,
                source: E::SOURCE, diagnostics: Vec::new() }));
        }
        for c in std::mem::take(&mut self.queued) {
            if let Cmd::Change { buffer_id, version, .. } = c {
                out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id, version,
                    source: E::SOURCE, diagnostics: Vec::new() }));
            }
        }
        out
    }
}

/// An LSP position value → `(line, utf16-character)`.
fn pos(v: &Value) -> Option<(u32, u32)> {
    Some((v.get("line")?.as_u64()? as u32, v.get("character")?.as_u64()? as u32))
}

/// The UTF-16 envelope (min start .. max end) over a raw LSP diagnostics array — the codeAction
/// query range. `None` if no diagnostic carries a well-formed range.
fn raw_envelope(raw: &[Value]) -> Option<((u32, u32), (u32, u32))> {
    let mut min_s: Option<(u32, u32)> = None;
    let mut max_e: Option<(u32, u32)> = None;
    for d in raw {
        let r = d.get("range")?;
        let s = pos(r.get("start")?)?;
        let e = pos(r.get("end")?)?;
        min_s = Some(min_s.map_or(s, |m| m.min(s)));
        max_e = Some(max_e.map_or(e, |m| m.max(e)));
    }
    Some((min_s?, max_e?))
}

/// A batched `textDocument/codeAction` request over `range`, carrying the publish's raw diagnostics
/// as context (the server's own positions — no round-trip conversion error, spec §3.3.5).
fn codeaction_request(id: u64, uri: &str, start: (u32, u32), end: (u32, u32), raw: &[Value]) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"textDocument/codeAction","params":{
        "textDocument":{"uri":uri},
        "range":{"start":{"line":start.0,"character":start.1},"end":{"line":end.0,"character":end.1}},
        "context":{"diagnostics": raw}
    }})
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
            if let Inbound::Cmd(Cmd::Change { buffer_id, version, .. }) = inb {
                let _ = self.msg_tx.send(Msg::DiagnosticsDone { buffer_id, version,
                    source: E::SOURCE, diagnostics: Vec::new() });
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
}
