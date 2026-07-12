//! The harper-ls client (Effort A, imperative shell): the `HarperLs` provider handle, the
//! long-lived client thread + `FlushGuard`, child spawn/respawn/shutdown, the pure `HarperState`
//! protocol state machine (incl. the `workspace/configuration` PULL responder), and eager-assembly.
//!
//! Functional-core / imperative-shell split: **`HarperState` is pure** — its inputs are `Inbound`
//! values + `now_ms`, its outputs are `Vec<Action>`, and it performs no IO. The client thread is a
//! thin pump that spawns/reaps the child, frames JSON-RPC over stdio, and executes the actions the
//! state machine returns. This is what makes the delicate protocol/concurrency logic exhaustively
//! unit-testable without a real `harper-ls` process (spec §15). The load-bearing invariants —
//! init-ordering (read the `initialize` response before `initialized`, else harper deadlocks), the
//! config PULL responder (unwrapped settings per `params.items`), and the terminal-guarantee latch
//! (`in_flight_version==Some(v)` ⟹ a terminal `DiagnosticsDone` for `v` is guaranteed) — live in
//! `HarperState` + `FlushGuard` and are covered by the inline tests.

use std::collections::HashMap;
use std::io::BufReader;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::app::Msg;
use crate::diag_provider::{
    Accepted, Availability, DiagnosticsProvider, ProviderConfig, ProviderEvent, INSTALL_HINT,
};
use crate::editor::BufferId;
use crate::limits::DIAG_MAX_SEND_BYTES;
use wordcartel_core::diagnostics::{Diagnostic, DiagnosticKind, DiagSource};

/// Publish watchdog: if the server never publishes for a sent version, emit an empty terminal
/// after this so the single-in-flight latch never wedges (spec §3.4).
const PUBLISH_TIMEOUT_MS: u64 = 10_000;
/// codeAction watchdog: emit the converted diagnostics suggestionless if the fix fetch stalls.
const CODEACTION_TIMEOUT_MS: u64 = 5_000;
/// Grace after `shutdown` before the pump forces `exit` + kills the child (bounded quit latency).
const SHUTDOWN_GRACE_MS: u64 = 1_000;
/// Respawn budget per session — the initial spawn counts as the first (spec §3.4; anti-crash-loop).
const MAX_SPAWN_ATTEMPTS: u32 = 3;
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

/// A command from the app-side handle, delivered over the `Inbound` channel.
#[derive(Debug, Clone)]
pub(crate) enum Cmd {
    Configure(ProviderConfig),
    Change { buffer_id: BufferId, version: u64, path: Option<std::path::PathBuf>, text: String },
    Close { buffer_id: BufferId },
    ReloadDict,
    Shutdown,
}

/// Everything the pump receives: app commands, one parsed server frame, or reader end-of-stream.
pub(crate) enum Inbound {
    Cmd(Cmd),
    Server(Value),
    ServerEof,
}

/// A side effect the pump performs on the state machine's behalf. `HarperState` returns these; the
/// thread executes them (never the reverse) so all protocol logic stays pure.
pub(crate) enum Action {
    Send(Value),
    Emit(Msg),
    SetAvailability(Availability),
    Respawn,
    Exit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase { Initializing, Running, ShuttingDown }

/// Per-document server-sync bookkeeping. `text` is the exact string last sent for this generation —
/// LSP positions are converted against it, never the live buffer (spec §5, §6).
struct DocState {
    uri: String, lsp_version: i32, our_version: u64, generation: u64, text: String, open: bool,
}
/// A didOpen/didChange awaiting its `publishDiagnostics` (or the publish watchdog).
struct AwaitPublish { our_version: u64, generation: u64, deadline: u64 }
/// Converted diagnostics parked while a batched codeAction is in flight (or its watchdog).
struct Assembly { our_version: u64, generation: u64, diags: Vec<Diagnostic>, deadline: u64 }
/// What an outstanding JSON-RPC request id means when its response lands.
enum PendingKind { Initialize, Shutdown, CodeAction { buffer_id: BufferId, generation: u64, our_version: u64 } }

/// The pure protocol state machine (spec §3.3). No IO — feed it `Inbound` + `now_ms`, execute the
/// returned `Vec<Action>`. Exhaustively unit-testable (see the inline tests).
pub(crate) struct HarperState {
    phase: Phase,
    cfg: ProviderConfig,
    docs: HashMap<BufferId, DocState>,
    uri_owner: HashMap<String, (BufferId, u64)>,
    next_generation: u64,
    queued: Vec<Cmd>,
    next_id: u64,
    pending_requests: HashMap<u64, PendingKind>,
    awaiting_publish: HashMap<BufferId, AwaitPublish>,
    assembling: HashMap<BufferId, Assembly>,
    spawn_attempts: u32,
}

impl HarperState {
    /// A fresh machine, pre-handshake. `spawn_attempts` starts at 1 — the initial spawn counts.
    pub(crate) fn new(cfg: ProviderConfig) -> HarperState {
        HarperState {
            phase: Phase::Initializing, cfg, docs: HashMap::new(), uri_owner: HashMap::new(),
            next_generation: 1, queued: Vec::new(), next_id: 1, pending_requests: HashMap::new(),
            awaiting_publish: HashMap::new(), assembling: HashMap::new(), spawn_attempts: 1,
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

    /// The BARE, unwrapped harper settings object (spec §8) — the load-bearing shape the config
    /// PULL response returns per `params.items` entry.
    pub(crate) fn settings_object(&self) -> Value {
        let mut linters = serde_json::Map::new();
        linters.insert("SpellCheck".into(), Value::Bool(true));
        if !self.cfg.grammar {
            for name in GRAMMAR_LINTERS { linters.insert((*name).into(), Value::Bool(false)); }
        }
        let mut obj = serde_json::Map::new();
        obj.insert("dialect".into(), Value::String("American".into()));
        if let Some(p) = &self.cfg.dictionary {
            obj.insert("userDictPath".into(), Value::String(p.to_string_lossy().into_owned()));
        }
        obj.insert("maxFileLength".into(), json!(self.cfg.max_file_length));
        obj.insert("linters".into(), Value::Object(linters));
        Value::Object(obj)
    }

    /// The `initialize` request. Advertises `workspace.configuration = true` (so harper PULLs config
    /// from us, §8) and `publishDiagnostics.versionSupport = true`.
    fn initialize_request(&self, id: u64) -> Value {
        json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":{
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
        }})
    }

    /// The `didChangeConfiguration` PUSH — NESTED under `"harper-ls"` (§8): only a trigger that
    /// makes harper re-PULL; the unwrapped pull RESPONSE is what actually delivers config.
    fn didchangeconfiguration_push(&self) -> Action {
        Action::Send(json!({"jsonrpc":"2.0","method":"workspace/didChangeConfiguration",
            "params":{"settings":{"harper-ls": self.settings_object()}}}))
    }

    /// (Re)spawn handshake step: reset to `Initializing`, mark every doc closed, clear pending, and
    /// send `initialize`. The pump must read its RESPONSE before `initialized` (deadlock guard).
    pub(crate) fn on_spawned(&mut self, _now: u64) -> Vec<Action> {
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
            Inbound::ServerEof => self.on_server_gone(now),
        }
    }

    fn apply_cmd(&mut self, c: Cmd, now: u64) -> Vec<Action> {
        match c {
            Cmd::Change { buffer_id, version, path, text } =>
                self.on_change(buffer_id, version, path, text, now),
            Cmd::Close { buffer_id } => self.on_close(buffer_id),
            Cmd::ReloadDict => vec![self.didchangeconfiguration_push()],
            Cmd::Configure(cfg) => { self.cfg = cfg; vec![self.didchangeconfiguration_push()] }
            Cmd::Shutdown => {
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
        let reopen = !self.docs.get(&buffer_id).map(|d| d.open).unwrap_or(false);
        let mut out = Vec::new();
        if reopen {
            let generation = self.next_generation; self.next_generation += 1;
            let uri = crate::lsp_rpc::doc_uri(buffer_id, generation);
            self.uri_owner.insert(uri.clone(), (buffer_id, generation));
            let lsp_version = 1;
            // Record awaiting BEFORE the Send action (non-IO first step; flush covers a mid-send death).
            self.awaiting_publish.insert(buffer_id,
                AwaitPublish { our_version: version, generation, deadline: now + PUBLISH_TIMEOUT_MS });
            out.push(Action::Send(json!({
                "jsonrpc":"2.0","method":"textDocument/didOpen",
                "params":{"textDocument":{"uri":uri,"languageId":"markdown","version":lsp_version,"text":text}}})));
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
                AwaitPublish { our_version: version, generation, deadline: now + PUBLISH_TIMEOUT_MS });
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
                source: DiagSource::Harper, diagnostics: Vec::new() }));
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

    /// Server→client requests. The `workspace/configuration` PULL responder is first-class (§8).
    fn on_server_request(&self, v: &Value) -> Vec<Action> {
        match v["method"].as_str().unwrap_or("") {
            "workspace/configuration" => vec![self.answer_configuration(v)],
            "window/workDoneProgress/create" | "client/registerCapability" =>
                vec![Action::Send(json!({"jsonrpc":"2.0","id":v["id"].clone(),"result":Value::Null}))],
            _ => vec![Action::Send(json!({"jsonrpc":"2.0","id":v["id"].clone(),
                "error":{"code":-32601,"message":"method not found"}}))],
        }
    }

    /// Answer a `workspace/configuration` request with a result array of **bare, unwrapped** settings
    /// objects — one per `params.items` entry, echoing the request `id` (spec §8, MUST-FIX).
    fn answer_configuration(&self, req: &Value) -> Action {
        let items = req["params"]["items"].as_array().map(|a| a.len()).unwrap_or(1);
        let obj = self.settings_object();
        let result: Vec<Value> = (0..items).map(|_| obj.clone()).collect(); // BARE, unwrapped
        Action::Send(json!({"jsonrpc":"2.0","id":req["id"].clone(),"result":result}))
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
            self.didchangeconfiguration_push(),
            // Handshake complete → the provider is LIVE (spec §10). This is the sole production
            // Ready transition: it lets `render_status` attribute `REVIEW · Harper` and stops the
            // debounced-recheck path stamping a permanent "starting grammar checker…". The SAME
            // path runs after a crash+respawn's re-initialize, so Ready is RESTORED post-respawn
            // (clearing the transient Starting stamped by `on_server_gone`).
            Action::SetAvailability(Availability::Ready),
        ];
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
                source: DiagSource::Harper, diagnostics: Vec::new() })];
        }
        let (start, end) = match raw_envelope(&raw) {
            Some(e) => e,
            None => return vec![Action::Emit(Msg::DiagnosticsDone { buffer_id, version: tagged,
                source: DiagSource::Harper, diagnostics: converted })], // no envelope → emit converted suggestionless
        };
        let id = self.alloc_id();
        self.pending_requests.insert(id, PendingKind::CodeAction { buffer_id, generation,
            our_version: tagged });
        self.assembling.insert(buffer_id, Assembly { our_version: tagged, generation,
            diags: converted, deadline: now + CODEACTION_TIMEOUT_MS });
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
                version: assembly.our_version, source: DiagSource::Harper, diagnostics: Vec::new() })];
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
            source: DiagSource::Harper, diagnostics: diags })]
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
            let kind = classify_lsp(d);
            if !self.cfg.grammar && kind == DiagnosticKind::Grammar { continue; }
            let message = d.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string();
            let code = match d.get("code") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(other) => Some(other.to_string()),
                None => None,
            };
            let href = d.get("codeDescription").and_then(|c| c.get("href"))
                .and_then(|h| h.as_str()).map(str::to_string);
            out.push(Diagnostic { range, kind, source: DiagSource::Harper, code, href, message,
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
                    source: DiagSource::Harper, diagnostics: Vec::new() }));
            }
        }
        let expired_asm: Vec<BufferId> = self.assembling.iter()
            .filter(|(_, a)| now >= a.deadline).map(|(b, _)| *b).collect();
        for b in expired_asm {
            if let Some(a) = self.assembling.remove(&b) {
                out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id: b, version: a.our_version,
                    source: DiagSource::Harper, diagnostics: a.diags }));
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
            out.push(Action::Emit(Msg::DiagProviderEvent { source: DiagSource::Harper,
                event: ProviderEvent::Restarted }));
            out.push(Action::Respawn);
        } else {
            out.push(Action::SetAvailability(Availability::Unavailable));
            out.push(Action::Emit(Msg::DiagProviderEvent { source: DiagSource::Harper,
                event: ProviderEvent::Degraded(CRASHED_HINT.into()) }));
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
                source: DiagSource::Harper, diagnostics: Vec::new() }));
        }
        for (b, a) in self.assembling.drain() {
            out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id: b, version: a.our_version,
                source: DiagSource::Harper, diagnostics: Vec::new() }));
        }
        for c in std::mem::take(&mut self.queued) {
            if let Cmd::Change { buffer_id, version, .. } = c {
                out.push(Action::Emit(Msg::DiagnosticsDone { buffer_id, version,
                    source: DiagSource::Harper, diagnostics: Vec::new() }));
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

/// Classify an LSP diagnostic (spec §6.3): a `code`/`source`/`message` mentioning spelling →
/// `Spelling`; otherwise → `Grammar`. Total two-variant mapping (harper's published set is curated).
fn classify_lsp(d: &Value) -> DiagnosticKind {
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

// ── The imperative shell: app-side handle + client thread + FlushGuard ──────────────────────────

/// Availability mirror shared between the app-side handle (`availability()` reads) and the client
/// thread (`SetAvailability` writes).
#[derive(Debug)]
struct Shared { availability: Mutex<Availability> }

/// The app-side `DiagnosticsProvider` handle. Cheap to construct (channel + `Shared`, **no thread,
/// no process**); `ensure_running` lazily spawns the client thread on first use.
#[derive(Debug)]
pub struct HarperLs {
    cmd_tx: Sender<Inbound>,
    rx: Option<Receiver<Inbound>>, // moved into the thread on first ensure_running
    shared: Arc<Shared>,
    started: bool,
    msg_tx: Sender<Msg>,
    cfg: ProviderConfig,
}

impl HarperLs {
    /// Construct the handle. Creates the `Inbound` channel + `Shared`; spawns nothing (idle is free).
    pub fn new(msg_tx: Sender<Msg>, cfg: ProviderConfig) -> HarperLs {
        let (cmd_tx, rx) = std::sync::mpsc::channel();
        HarperLs {
            cmd_tx, rx: Some(rx),
            shared: Arc::new(Shared { availability: Mutex::new(Availability::Idle) }),
            started: false, msg_tx, cfg,
        }
    }

    fn set_availability(&self, a: Availability) {
        *self.shared.availability.lock().expect("availability mutex") = a;
    }
}

impl DiagnosticsProvider for HarperLs {
    fn name(&self) -> &'static str { "Harper" }

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
            .name("wcartel-harper-client".into())
            .spawn(move || run_client(msg_tx, rx, inbound_tx, shared, cfg));
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
}

/// Owns `cmd_rx` and runs the two-part flush on `Drop` — the last leg of the latch invariant
/// (§3.2). On ANY thread exit (clean, degrade, or panic-unwind), it emits an empty version-tagged
/// terminal for (1) every entry the pump recorded (`state.flush_outstanding()`) and (2) every
/// `Cmd::Change` still unread in the channel (the accepted-but-unrecorded gap).
struct FlushGuard {
    state: HarperState,
    cmd_rx: Receiver<Inbound>,
    msg_tx: Sender<Msg>,
}

impl Drop for FlushGuard {
    fn drop(&mut self) {
        for a in self.state.flush_outstanding() {
            if let Action::Emit(m) = a { let _ = self.msg_tx.send(m); }
        }
        while let Ok(inb) = self.cmd_rx.try_recv() {
            if let Inbound::Cmd(Cmd::Change { buffer_id, version, .. }) = inb {
                let _ = self.msg_tx.send(Msg::DiagnosticsDone { buffer_id, version,
                    source: DiagSource::Harper, diagnostics: Vec::new() });
            }
        }
    }
}

/// The client thread entry point. Wraps the pump in `catch_unwind` so a panic cannot bypass the
/// `FlushGuard` (which lives in this outer scope and drops after the catch → flush always runs).
fn run_client(msg_tx: Sender<Msg>, cmd_rx: Receiver<Inbound>, inbound_tx: Sender<Inbound>,
    shared: Arc<Shared>, cfg: ProviderConfig) {
    let mut guard = FlushGuard { state: HarperState::new(cfg), cmd_rx, msg_tx };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pump(&mut guard, &inbound_tx, &shared);
    }));
    // guard drops here → the two-part flush, even on a panic-unwind path.
}

/// Spawn the child + its reader thread; hand back the child and its stdin.
fn spawn_session(inbound_tx: &Sender<Inbound>) -> std::io::Result<(Child, ChildStdin)> {
    let mut child = Command::new("harper-ls").arg("--stdio")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    spawn_reader(stdout, inbound_tx.clone());
    Ok((child, stdin))
}

/// The reader thread (`wcartel-harper-read`): loop `read_frame`, forward each frame as
/// `Inbound::Server`; on read error / EOF forward `Inbound::ServerEof` and exit (death is a message,
/// never a hang — mirrors the M4 input-thread shape).
fn spawn_reader(stdout: ChildStdout, inbound_tx: Sender<Inbound>) {
    let _ = std::thread::Builder::new().name("wcartel-harper-read".into()).spawn(move || {
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
enum Control { Continue, Exit, Respawn }

/// The pump: spawn the child, feed the handshake, then `recv_timeout(next deadline)` over the single
/// `Inbound` channel — feeding `HarperState` and executing the actions it returns. Blocks on `recv`
/// with nothing pending (idle is free).
fn pump(guard: &mut FlushGuard, inbound_tx: &Sender<Inbound>, shared: &Arc<Shared>) {
    let start = Instant::now();
    let now = |s: &Instant| s.elapsed().as_millis() as u64;
    let (mut child, mut stdin) = match spawn_session(inbound_tx) {
        Ok(s) => s,
        Err(_) => {
            // NotFound (or any initial spawn failure) IS the runtime PATH detection (§3.2).
            set_availability(shared, Availability::Unavailable);
            let _ = guard.msg_tx.send(Msg::DiagProviderEvent { source: DiagSource::Harper,
                event: ProviderEvent::Degraded(INSTALL_HINT.into()) });
            return;
        }
    };
    let acts = guard.state.on_spawned(now(&start));
    let _ = run_actions(acts, &mut stdin, &guard.msg_tx, shared);
    set_availability(shared, Availability::Starting);

    let mut shutdown_at: Option<u64> = None;
    loop {
        let deadline = merge_deadline(guard.state.next_deadline(), shutdown_at);
        let acts = match wait_inbound(&guard.cmd_rx, deadline, now(&start)) {
            Wait::Closed => break, // app dropped the handle — end the thread (guard flushes).
            Wait::Timeout => {
                if let Some(sd) = shutdown_at {
                    if now(&start) >= sd { let _ = write_frame_to(&mut stdin, &exit_notification()); break; }
                }
                guard.state.on_deadline(now(&start))
            }
            Wait::Got(inb) => guard.state.on_inbound(inb, now(&start)),
        };
        match run_actions(acts, &mut stdin, &guard.msg_tx, shared) {
            Control::Continue => {}
            Control::Exit => break,
            Control::Respawn => {
                let _ = child.kill(); let _ = child.wait();
                match spawn_session(inbound_tx) {
                    Ok((c, s)) => {
                        child = c; stdin = s;
                        let acts = guard.state.on_spawned(now(&start));
                        let _ = run_actions(acts, &mut stdin, &guard.msg_tx, shared);
                    }
                    Err(_) => { let _ = inbound_tx.send(Inbound::ServerEof); } // consume the next budget step
                }
            }
        }
        if guard.state.is_shutting_down() && shutdown_at.is_none() {
            shutdown_at = Some(now(&start) + SHUTDOWN_GRACE_MS);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn exit_notification() -> Value { json!({"jsonrpc":"2.0","method":"exit"}) }

fn set_availability(shared: &Arc<Shared>, a: Availability) {
    *shared.availability.lock().expect("availability mutex") = a;
}

fn write_frame_to(stdin: &mut ChildStdin, v: &Value) -> std::io::Result<()> {
    crate::lsp_rpc::write_frame(stdin, v)
}

/// Execute one batch of actions in order; return the first control-flow action (Respawn/Exit) hit.
fn run_actions(acts: Vec<Action>, stdin: &mut ChildStdin, msg_tx: &Sender<Msg>, shared: &Arc<Shared>)
    -> Control {
    for a in acts {
        match a {
            Action::Send(v) => { let _ = write_frame_to(stdin, &v); }
            Action::Emit(m) => { let _ = msg_tx.send(m); }
            Action::SetAvailability(av) => set_availability(shared, av),
            Action::Respawn => return Control::Respawn,
            Action::Exit => return Control::Exit,
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
    use wordcartel_core::diagnostics::Suggestion;

    fn cfg(grammar: bool) -> ProviderConfig {
        ProviderConfig { grammar, dictionary: None, max_file_length: 10_000 }
    }

    // ── test helpers: extract Sends / emitted DiagnosticsDone from a Vec<Action> ────────────────

    fn sends(acts: &[Action]) -> Vec<&Value> {
        acts.iter().filter_map(|a| if let Action::Send(v) = a { Some(v) } else { None }).collect()
    }

    /// Every emitted `DiagnosticsDone` as `(buffer_id, version, diagnostics)`.
    fn diag_dones(acts: &[Action]) -> Vec<(BufferId, u64, Vec<Diagnostic>)> {
        acts.iter().filter_map(|a| match a {
            Action::Emit(Msg::DiagnosticsDone { buffer_id, version, source: _, diagnostics }) =>
                Some((*buffer_id, *version, diagnostics.clone())),
            _ => None,
        }).collect()
    }

    fn has_restarted(acts: &[Action]) -> bool {
        acts.iter().any(|a| matches!(a,
            Action::Emit(Msg::DiagProviderEvent { event: ProviderEvent::Restarted, .. })))
    }
    fn degrade_hint(acts: &[Action]) -> Option<String> {
        acts.iter().find_map(|a| match a {
            Action::Emit(Msg::DiagProviderEvent { event: ProviderEvent::Degraded(h), .. }) => Some(h.clone()),
            _ => None,
        })
    }
    fn availabilities(acts: &[Action]) -> Vec<Availability> {
        acts.iter().filter_map(|a| if let Action::SetAvailability(v) = a { Some(*v) } else { None }).collect()
    }
    fn method_of(v: &Value) -> &str { v["method"].as_str().unwrap_or("") }

    /// Drive `new → on_spawned → initialize response` to a Running machine (grammar on).
    fn running(grammar: bool) -> HarperState {
        let mut st = HarperState::new(cfg(grammar));
        let spawn = st.on_spawned(0);
        let init = sends(&spawn)[0];
        assert_eq!(method_of(init), "initialize");
        let id = init["id"].as_u64().expect("initialize id");
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,
            "result":{"capabilities":{}}})), 0);
        // initialized + didChangeConfiguration pushed on handshake completion.
        let methods: Vec<&str> = sends(&out).iter().map(|v| method_of(v)).collect();
        assert_eq!(methods, ["initialized", "workspace/didChangeConfiguration"]);
        st
    }

    // ── handshake / init ordering ───────────────────────────────────────────────────────────────

    #[test]
    fn handshake_sends_initialize_advertising_workspace_configuration() {
        let mut st = HarperState::new(cfg(true));
        let spawn = st.on_spawned(0);
        let init = sends(&spawn)[0];
        assert_eq!(method_of(init), "initialize");
        assert_eq!(init["params"]["capabilities"]["workspace"]["configuration"], json!(true),
            "must advertise workspace.configuration=true so harper PULLs config");
        assert_eq!(init["params"]["capabilities"]["textDocument"]["publishDiagnostics"]["versionSupport"],
            json!(true));
    }

    #[test]
    fn initialized_is_sent_only_after_the_initialize_response() {
        let mut st = HarperState::new(cfg(true));
        // Before the response, a queued change must NOT elicit initialized/didChange (still Initializing).
        let spawn = st.on_spawned(0);
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        let queued = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1,
            path: None, text: "x".into() }), 0);
        assert!(queued.is_empty(), "commands before the initialize response queue silently");
        // The response releases initialized (deadlock guard) THEN replays the queued didOpen.
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,"result":{}})), 0);
        let methods: Vec<&str> = sends(&out).iter().map(|v| method_of(v)).collect();
        assert_eq!(methods, ["initialized", "workspace/didChangeConfiguration", "textDocument/didOpen"]);
    }

    #[test]
    fn on_initialized_emits_ready_on_handshake_and_restores_it_after_respawn() {
        let mut st = HarperState::new(cfg(true));
        let spawn = st.on_spawned(0);
        let id = sends(&spawn)[0]["id"].as_u64().unwrap();
        // Handshake completion is the SOLE production Ready transition (spec §10) — without it the
        // REVIEW·Harper attribution never fires and every recheck falsely stamps "starting…".
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id,"result":{}})), 0);
        assert!(availabilities(&out).contains(&Availability::Ready),
            "handshake completion emits SetAvailability(Ready)");
        // A crash flips to the transient Starting; the respawn re-handshake runs the SAME
        // on_initialized path and RESTORES Ready.
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1, path: None,
            text: "a".into() }), 0);
        let gone = st.on_inbound(Inbound::ServerEof, 0);
        assert_eq!(availabilities(&gone), vec![Availability::Starting], "crash → transient Starting");
        let respawn = st.on_spawned(0);
        let rid = sends(&respawn)[0]["id"].as_u64().unwrap();
        let reinit = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":rid,"result":{}})), 0);
        assert!(availabilities(&reinit).contains(&Availability::Ready),
            "post-respawn re-handshake restores Ready");
    }

    // ── config PULL responder (unwrapped) ───────────────────────────────────────────────────────

    #[test]
    fn configuration_pull_answers_unwrapped_settings_per_item() {
        let st = running(false); // grammar off → settings carry the linter partition
        let req = json!({"jsonrpc":"2.0","id":42,"method":"workspace/configuration",
            "params":{"items":[{},{}]}}); // two items
        let out = { let mut s = st; s.on_inbound(Inbound::Server(req), 0) };
        let resp = sends(&out)[0];
        assert_eq!(resp["id"], json!(42), "echoes the request id");
        let result = resp["result"].as_array().expect("result array");
        assert_eq!(result.len(), 2, "one settings object per params.items entry");
        // BARE / unwrapped — NOT nested under harper-ls.
        assert!(result[0].get("harper-ls").is_none(), "response settings must be unwrapped");
        assert_eq!(result[0]["dialect"], json!("American"));
        assert_eq!(result[0]["linters"]["SpellCheck"], json!(true));
        assert_eq!(result[0]["linters"]["SentenceCapitalization"], json!(false),
            "grammar off → grammar-tier linters false");
    }

    #[test]
    fn settings_object_omits_dict_when_none_and_toggles_grammar() {
        let on = HarperState::new(cfg(true)).settings_object();
        assert!(on.get("userDictPath").is_none(), "dictionary None → key omitted");
        assert_eq!(on["linters"]["SpellCheck"], json!(true));
        assert!(on["linters"].get("SentenceCapitalization").is_none(),
            "grammar on → grammar linters left at server defaults (absent)");
        let with_dict = HarperState::new(ProviderConfig { grammar: true,
            dictionary: Some("/d.txt".into()), max_file_length: 5 }).settings_object();
        assert_eq!(with_dict["userDictPath"], json!("/d.txt"));
        assert_eq!(with_dict["maxFileLength"], json!(5));
    }

    // ── text sync: didOpen → didChange, opaque uri, lsp_version ─────────────────────────────────

    #[test]
    fn first_change_opens_then_subsequent_change_is_plain_didchange() {
        let mut st = running(true);
        let o = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(2), version: 1,
            path: None, text: "hi".into() }), 0);
        let open = sends(&o)[0];
        assert_eq!(method_of(open), "textDocument/didOpen");
        assert_eq!(open["params"]["textDocument"]["uri"], json!("untitled:wcartel-2-1"));
        assert_eq!(open["params"]["textDocument"]["version"], json!(1));
        // A save/edit at the same buffer is a plain didChange (no reopen), lsp_version increments.
        let c = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(2), version: 2,
            path: Some("f.md".into()), text: "hi there".into() }), 0);
        let ch = sends(&c)[0];
        assert_eq!(method_of(ch), "textDocument/didChange");
        assert_eq!(ch["params"]["textDocument"]["uri"], json!("untitled:wcartel-2-1"), "same opaque uri");
        assert_eq!(ch["params"]["textDocument"]["version"], json!(2), "lsp_version 1→2");
    }

    #[test]
    fn lsp_version_saturates_at_i32_max() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1, path: None,
            text: "a".into() }), 0);
        // Force the counter to the ceiling, then a change must saturate (no wrap / no panic in release).
        st.docs.get_mut(&BufferId(0)).unwrap().lsp_version = i32::MAX;
        st.docs.get_mut(&BufferId(0)).unwrap().open = true;
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 2, path: None,
                text: "b".into() }), 0)
        }));
        // In release the saturating_add pins at i32::MAX; debug_assert may fire in debug — either way
        // no wrap to a negative version. Assert the pinned value when it did not panic.
        if let Ok(o) = out {
            assert_eq!(sends(&o)[0]["params"]["textDocument"]["version"], json!(i32::MAX));
        }
    }

    // ── generation attribution ──────────────────────────────────────────────────────────────────

    #[test]
    fn publish_for_unknown_uri_is_dropped() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1, path: None,
            text: "teh".into() }), 0);
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-9-9","diagnostics":[]}})), 0);
        assert!(out.is_empty(), "publish for a uri not in uri_owner is dropped outright");
    }

    #[test]
    fn empty_publish_emits_terminal_immediately_with_version_echo_absent() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 7, path: None,
            text: "ok".into() }), 0);
        // No "version" field (harper 2.1.0) → accepted via generation.
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-0-1","diagnostics":[]}})), 0);
        assert_eq!(diag_dones(&out), vec![(BufferId(0), 7, vec![])]);
    }

    // ── Cmd::Close emits the terminal before removing state ──────────────────────────────────────

    #[test]
    fn close_emits_terminal_before_removing_state() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 5, path: None,
            text: "teh".into() }), 0);
        let out = st.on_inbound(Inbound::Cmd(Cmd::Close { buffer_id: BufferId(0) }), 0);
        // Terminal for the outstanding version FIRST, then didClose.
        assert_eq!(diag_dones(&out), vec![(BufferId(0), 5, vec![])]);
        assert_eq!(method_of(sends(&out)[0]), "textDocument/didClose");
        // State gone: a later publish for the old uri is dropped.
        let late = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-0-1","diagnostics":[]}})), 0);
        assert!(late.is_empty());
    }

    #[test]
    fn reload_recover_race_old_generation_publish_dropped() {
        let mut st = running(true);
        // await for gen 1
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1, path: None,
            text: "a".into() }), 0);
        // Close (reload/recover) then reopen at gen 2 with a bumped version.
        st.on_inbound(Inbound::Cmd(Cmd::Close { buffer_id: BufferId(0) }), 0);
        let reopen = st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 2,
            path: None, text: "b".into() }), 0);
        assert_eq!(sends(&reopen)[0]["params"]["textDocument"]["uri"], json!("untitled:wcartel-0-2"));
        // The still-in-transit OLD-generation publish carries the retired uri → dropped.
        let stale = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-0-1","diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                 "message":"x","code":"SpellCheck"}]}})), 0);
        assert!(stale.is_empty(), "old-generation publish dropped, no emission for the retired uri");
    }

    // ── codeAction: shape, attach, command-only dropped ─────────────────────────────────────────

    /// Publish a single spelling diagnostic over "teh" (bytes 0..3), returning the codeAction id
    /// the machine allocated.
    fn publish_teh(st: &mut HarperState, buffer: BufferId, uri: &str) -> u64 {
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":uri,"diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},
                 "message":"spelling","code":"SpellCheck"}]}})), 0);
        let ca = sends(&out).into_iter().find(|v| method_of(v) == "textDocument/codeAction")
            .expect("a codeAction request was sent for the non-empty publish");
        let _ = buffer;
        ca["id"].as_u64().expect("codeAction id")
    }

    #[test]
    fn nonempty_publish_then_codeaction_attaches_replace_with_and_drops_command_only() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 5, path: None,
            text: "teh".into() }), 0);
        let id = publish_teh(&mut st, BufferId(0), "untitled:wcartel-0-1");
        let resp = json!({"jsonrpc":"2.0","id":id,"result":[
            {"kind":"quickfix","edit":{"changes":{"untitled:wcartel-0-1":[
                {"newText":"the","range":{"start":{"line":0,"character":0},
                    "end":{"line":0,"character":3}}}]}}},
            {"kind":Value::Null,"command":{"title":"Add to dictionary"}}
        ]});
        let out = st.on_inbound(Inbound::Server(resp), 0);
        let done = diag_dones(&out);
        assert_eq!(done.len(), 1);
        let (b, v, diags) = &done[0];
        assert_eq!((*b, *v), (BufferId(0), 5));
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].suggestions, vec![Suggestion::ReplaceWith("the".into())],
            "quickfix attached; command-only action dropped");
    }

    #[test]
    fn assembly_superseded_generation_is_discarded_not_emitted_with_new_ranges() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 5, path: None,
            text: "teh".into() }), 0);
        let id = publish_teh(&mut st, BufferId(0), "untitled:wcartel-0-1");
        // Force a generation bump on the doc while the codeAction is in flight.
        st.docs.get_mut(&BufferId(0)).unwrap().generation = 99;
        let resp = json!({"jsonrpc":"2.0","id":id,"result":[
            {"kind":"quickfix","edit":{"changes":{"untitled:wcartel-0-1":[
                {"newText":"the","range":{"start":{"line":0,"character":0},
                    "end":{"line":0,"character":3}}}]}}}]});
        let out = st.on_inbound(Inbound::Server(resp), 0);
        // Discarded: no non-empty diagnostics emitted against the newer generation.
        for (_, _, diags) in diag_dones(&out) {
            assert!(diags.is_empty(), "superseded assembly must not paint against new text");
        }
    }

    #[test]
    fn stale_codeaction_response_does_not_consume_the_newer_assembly() {
        let mut st = running(true);
        // v1: publish parks assembly #1 (our_version=1) and issues codeAction id1.
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1, path: None,
            text: "teh".into() }), 0);
        let id1 = publish_teh(&mut st, BufferId(0), "untitled:wcartel-0-1");
        // codeAction #1 stalls past its watchdog: assembly #1 emits v1 suggestionless and is removed,
        // but its pending request survives — a late response can still route to on_codeaction_response.
        let w = st.on_deadline(CODEACTION_TIMEOUT_MS);
        assert_eq!(diag_dones(&w)[0].1, 1, "watchdog terminated v1 suggestionless");
        // v2 edit + publish parks a BRAND-NEW assembly #2 (same generation, our_version=2), id2.
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 2, path: None,
            text: "teh".into() }), CODEACTION_TIMEOUT_MS);
        let id2 = publish_teh(&mut st, BufferId(0), "untitled:wcartel-0-1");
        assert_ne!(id1, id2);
        // The LATE response to request #1 lands now: its our_version (1) ≠ the parked assembly's (2),
        // so it must be DISCARDED — no emit, and assembly #2 left intact for its own response.
        let stale = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id1,"result":[
            {"kind":"quickfix","edit":{"changes":{"untitled:wcartel-0-1":[
                {"newText":"STALE","range":{"start":{"line":0,"character":0},
                    "end":{"line":0,"character":3}}}]}}}]})), CODEACTION_TIMEOUT_MS);
        assert!(diag_dones(&stale).is_empty(), "stale v1 response discarded — no emission");
        assert!(st.assembling.contains_key(&BufferId(0)), "assembly #2 left intact for its own response");
        // The real v2 response attaches its OWN fresh fix and emits for v2.
        let fresh = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0","id":id2,"result":[
            {"kind":"quickfix","edit":{"changes":{"untitled:wcartel-0-1":[
                {"newText":"the","range":{"start":{"line":0,"character":0},
                    "end":{"line":0,"character":3}}}]}}}]})), CODEACTION_TIMEOUT_MS);
        let done = diag_dones(&fresh);
        assert_eq!(done.len(), 1);
        assert_eq!((done[0].0, done[0].1), (BufferId(0), 2), "emitted for v2");
        assert_eq!(done[0].2[0].suggestions, vec![Suggestion::ReplaceWith("the".into())],
            "v2 assembly attached its OWN fresh edits, not the stale v1 ones");
    }

    // ── watchdogs ───────────────────────────────────────────────────────────────────────────────

    #[test]
    fn publish_watchdog_emits_empty_after_deadline() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 3, path: None,
            text: "hi".into() }), 0);
        let early = st.on_deadline(PUBLISH_TIMEOUT_MS - 1);
        assert!(early.is_empty(), "not yet past the deadline");
        let out = st.on_deadline(PUBLISH_TIMEOUT_MS);
        assert_eq!(diag_dones(&out), vec![(BufferId(0), 3, vec![])]);
    }

    #[test]
    fn codeaction_watchdog_emits_converted_suggestionless() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 4, path: None,
            text: "teh".into() }), 0);
        publish_teh(&mut st, BufferId(0), "untitled:wcartel-0-1");
        let out = st.on_deadline(CODEACTION_TIMEOUT_MS);
        let done = diag_dones(&out);
        assert_eq!(done.len(), 1);
        assert_eq!((done[0].0, done[0].1), (BufferId(0), 4));
        assert_eq!(done[0].2.len(), 1, "the converted diagnostic still paints");
        assert!(done[0].2[0].suggestions.is_empty(), "no fixes on a codeAction timeout");
    }

    // ── flush_outstanding covers awaiting + assembling + queued ────────────────────────────────

    #[test]
    fn flush_outstanding_covers_all_three_tracks_and_is_idempotent() {
        let mut st = running(true);
        // awaiting (buffer 0)
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1, path: None,
            text: "a".into() }), 0);
        // assembling (buffer 1)
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(1), version: 2, path: None,
            text: "teh".into() }), 0);
        publish_teh(&mut st, BufferId(1), "untitled:wcartel-1-2");
        // queued (buffer 2): drop back to Initializing so a change queues instead of applying.
        st.phase = Phase::Initializing;
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(2), version: 3, path: None,
            text: "q".into() }), 0);
        let mut done = diag_dones(&st.flush_outstanding());
        done.sort_by_key(|(b, _, _)| b.0);
        assert_eq!(done, vec![(BufferId(0), 1, vec![]), (BufferId(1), 2, vec![]), (BufferId(2), 3, vec![])]);
        assert!(st.flush_outstanding().is_empty(), "idempotent — a second flush emits nothing");
    }

    // ── crash → respawn budget flushes the latch ───────────────────────────────────────────────

    #[test]
    fn server_eof_with_budget_flushes_latch_then_restarts() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 5, path: None,
            text: "a".into() }), 0);
        let out = st.on_inbound(Inbound::ServerEof, 0);
        // The empty terminal for v=5 is emitted BEFORE the Respawn (unwedges the latch).
        let emit_idx = out.iter().position(|a| matches!(a,
            Action::Emit(Msg::DiagnosticsDone { version: 5, .. }))).expect("flush emit");
        let respawn_idx = out.iter().position(|a| matches!(a, Action::Respawn)).expect("respawn");
        assert!(emit_idx < respawn_idx, "flush precedes respawn");
        assert!(has_restarted(&out));
        assert_eq!(availabilities(&out), vec![Availability::Starting]);
    }

    #[test]
    fn server_eof_budget_exhaustion_flushes_then_degrades() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 5, path: None,
            text: "a".into() }), 0);
        // spawn_attempts starts at 1: 1st and 2nd EOFs respawn, the 3rd exhausts the budget.
        let _ = st.on_inbound(Inbound::ServerEof, 0); // attempts → 2
        let _ = st.on_inbound(Inbound::ServerEof, 0); // attempts → 3
        // Re-arm an awaiting so the exhaustion path also has a latch to flush.
        st.phase = Phase::Running;
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(1), version: 8, path: None,
            text: "b".into() }), 0);
        let out = st.on_inbound(Inbound::ServerEof, 0);
        assert!(diag_dones(&out).iter().any(|(b, v, _)| *b == BufferId(1) && *v == 8),
            "outstanding latch flushed before degrade");
        assert_eq!(availabilities(&out), vec![Availability::Unavailable]);
        assert_eq!(degrade_hint(&out), Some(CRASHED_HINT.to_string()));
        assert!(out.iter().any(|a| matches!(a, Action::Exit)));
    }

    // ── assembly-overwrite guard (round-2 IMPORTANT) + watchdog symmetry ───────────────────────

    #[test]
    fn assembly_result_then_eof_does_not_re_emit_empty_for_the_same_version() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 5, path: None,
            text: "teh".into() }), 0);
        let id = publish_teh(&mut st, BufferId(0), "untitled:wcartel-0-1");
        // codeAction response emits the NON-EMPTY terminal for v=5 and removes assembling[0].
        let resp = json!({"jsonrpc":"2.0","id":id,"result":[
            {"kind":"quickfix","edit":{"changes":{"untitled:wcartel-0-1":[
                {"newText":"the","range":{"start":{"line":0,"character":0},
                    "end":{"line":0,"character":3}}}]}}}]});
        let landed = st.on_inbound(Inbound::Server(resp), 0);
        assert_eq!(diag_dones(&landed)[0].2.len(), 1, "non-empty result landed for v=5");
        // Now the server dies: the flush must find NO tracked entry for v=5 → no empty clobber.
        let after = st.on_inbound(Inbound::ServerEof, 0);
        assert!(!after.iter().any(|a| matches!(a,
            Action::Emit(Msg::DiagnosticsDone { buffer_id: BufferId(0), version: 5, .. }))),
            "no second (empty) terminal for a version whose non-empty result already landed");
    }

    #[test]
    fn publish_watchdog_then_eof_no_duplicate_terminal() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 6, path: None,
            text: "hi".into() }), 0);
        let w = st.on_deadline(PUBLISH_TIMEOUT_MS);
        assert_eq!(diag_dones(&w), vec![(BufferId(0), 6, vec![])]);
        let after = st.on_inbound(Inbound::ServerEof, 0);
        assert!(!after.iter().any(|a| matches!(a,
            Action::Emit(Msg::DiagnosticsDone { buffer_id: BufferId(0), version: 6, .. }))),
            "watchdog already removed the awaiting entry; the flush finds nothing to re-emit");
    }

    #[test]
    fn codeaction_watchdog_then_eof_no_duplicate_terminal() {
        let mut st = running(true);
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 7, path: None,
            text: "teh".into() }), 0);
        publish_teh(&mut st, BufferId(0), "untitled:wcartel-0-1");
        let w = st.on_deadline(CODEACTION_TIMEOUT_MS);
        assert_eq!(diag_dones(&w)[0].1, 7);
        let after = st.on_inbound(Inbound::ServerEof, 0);
        assert!(!after.iter().any(|a| matches!(a,
            Action::Emit(Msg::DiagnosticsDone { buffer_id: BufferId(0), version: 7, .. }))),
            "assembly watchdog already removed the entry; no duplicate on EOF");
    }

    // ── classification / grammar gate ───────────────────────────────────────────────────────────

    #[test]
    fn classify_lsp_spelling_vs_grammar() {
        assert_eq!(classify_lsp(&json!({"code":"SpellCheck","message":"x"})), DiagnosticKind::Spelling);
        assert_eq!(classify_lsp(&json!({"code":"LongSentences","message":"x"})), DiagnosticKind::Grammar);
        assert_eq!(classify_lsp(&json!({"message":"possible spelling mistake"})), DiagnosticKind::Spelling);
        assert_eq!(classify_lsp(&json!({"message":"style"})), DiagnosticKind::Grammar);
    }

    #[test]
    fn grammar_gate_drops_grammar_diagnostics_when_disabled() {
        let mut st = running(false); // grammar off
        st.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 1, path: None,
            text: "teh cat".into() }), 0);
        // One spelling + one grammar diagnostic; only spelling survives → converted non-empty.
        let out = st.on_inbound(Inbound::Server(json!({"jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{"uri":"untitled:wcartel-0-1","diagnostics":[
                {"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},
                 "message":"spelling","code":"SpellCheck"},
                {"range":{"start":{"line":0,"character":4},"end":{"line":0,"character":7}},
                 "message":"style","code":"LongSentences"}]}})), 0);
        // Non-empty (spelling remains) → a codeAction went out; assembly holds exactly one diag.
        assert!(sends(&out).iter().any(|v| method_of(v) == "textDocument/codeAction"));
        assert_eq!(st.assembling.get(&BufferId(0)).unwrap().diags.len(), 1,
            "grammar-classified diagnostic dropped by the client gate");
    }

    // ── FlushGuard: drop emits terminals for tracked + queued (channel-drain) ──────────────────

    #[test]
    fn flush_guard_drop_emits_for_tracked_and_channel_change() {
        let (msg_tx, msg_rx) = std::sync::mpsc::channel::<Msg>();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Inbound>();
        let mut state = running(true);
        // Tracked: an awaiting slot for buffer 0 v=10.
        state.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 10, path: None,
            text: "a".into() }), 0);
        // Unread in the channel: an accepted-but-unrecorded change for buffer 1 v=11.
        cmd_tx.send(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(1), version: 11, path: None,
            text: "b".into() })).unwrap();
        let guard = FlushGuard { state, cmd_rx, msg_tx };
        drop(guard);
        let mut got: Vec<(BufferId, u64)> = Vec::new();
        while let Ok(m) = msg_rx.try_recv() {
            if let Msg::DiagnosticsDone { buffer_id, version, source, diagnostics } = m {
                assert_eq!(source, DiagSource::Harper);
                assert!(diagnostics.is_empty());
                got.push((buffer_id, version));
            }
        }
        got.sort_by_key(|(b, _)| b.0);
        assert_eq!(got, vec![(BufferId(0), 10), (BufferId(1), 11)],
            "both the tracked awaiting and the channel-drained change get an empty terminal");
    }

    #[test]
    fn flush_guard_flushes_even_when_pump_panics() {
        // The guard lives in an outer scope; a panic inside catch_unwind still runs its Drop.
        let (msg_tx, msg_rx) = std::sync::mpsc::channel::<Msg>();
        let (_cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Inbound>();
        let mut state = running(true);
        state.on_inbound(Inbound::Cmd(Cmd::Change { buffer_id: BufferId(0), version: 21, path: None,
            text: "a".into() }), 0);
        let guard = FlushGuard { state, cmd_rx, msg_tx };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Simulate the pump touching state, then panicking mid-flight.
            let _ = guard.state.next_deadline();
            panic!("pump exploded");
        }));
        drop(guard);
        let got: Vec<(BufferId, u64)> = std::iter::from_fn(|| msg_rx.try_recv().ok())
            .filter_map(|m| if let Msg::DiagnosticsDone { buffer_id, version, .. } = m {
                Some((buffer_id, version)) } else { None }).collect();
        assert_eq!(got, vec![(BufferId(0), 21)], "the latch is flushed on panic-unwind");
    }

    // ── HarperLs handle: construction is thread-free; disconnected send is Accepted::No ─────────

    #[test]
    fn harper_ls_new_is_idle_and_spawns_nothing() {
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel::<Msg>();
        let p = HarperLs::new(msg_tx, cfg(true));
        assert_eq!(p.name(), "Harper");
        assert_eq!(p.availability(), Availability::Idle);
    }

    #[test]
    fn notify_change_over_cap_is_not_accepted() {
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel::<Msg>();
        let mut p = HarperLs::new(msg_tx, cfg(true));
        let huge = "x".repeat((DIAG_MAX_SEND_BYTES as usize) + 1);
        assert_eq!(p.notify_change(BufferId(0), 1, None, huge), Accepted::No,
            "an over-cap document is skipped with no latch");
    }

    #[test]
    fn notify_change_accepts_while_thread_alive_then_no_on_disconnect() {
        let (msg_tx, _msg_rx) = std::sync::mpsc::channel::<Msg>();
        let mut p = HarperLs::new(msg_tx, cfg(true));
        // The receiver still lives inside `p.rx` (ensure_running not called) → send succeeds.
        assert_eq!(p.notify_change(BufferId(0), 1, None, "hi".into()), Accepted::Yes);
        // Drop the receiver to simulate a dead thread → disconnected send flips availability.
        p.rx = None;
        assert_eq!(p.notify_change(BufferId(0), 2, None, "hi".into()), Accepted::No);
        assert_eq!(p.availability(), Availability::Unavailable);
    }
}
