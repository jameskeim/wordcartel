//! The harper-ls engine (Effort A; E10 T1): `HarperEngine` — harper's identity, timeouts,
//! settings PULL/push shapes, and classifier — over the engine-generic `lsp_client` core.
//! The protocol state machine, pump, and `FlushGuard` moved to `lsp_client.rs` verbatim
//! (spec 2026-07-25-e10 §3); the inline test module below is the extraction PIN and is
//! byte-for-byte identical to its pre-extraction form.
use serde_json::{json, Value};

// Test-surface re-exports (T1 census): the inline test module reaches these via `use super::*`.
// Names this module's own production code also needs (ProviderConfig, DiagnosticKind, DiagSource)
// are re-exported unconditionally; names ONLY the pinned test module touches are re-exported under
// `#[cfg(test)]` — a `pub(crate) use` that stays unused outside cfg(test) still trips unused_imports.
pub(crate) use crate::diag_provider::ProviderConfig;
pub(crate) use wordcartel_core::diagnostics::{DiagnosticKind, DiagSource};
#[cfg(test)]
pub(crate) use crate::app::Msg;
#[cfg(test)]
pub(crate) use crate::diag_provider::{Accepted, Availability, DiagnosticsProvider, ProviderEvent};
#[cfg(test)]
pub(crate) use crate::editor::BufferId;
#[cfg(test)]
pub(crate) use crate::limits::DIAG_MAX_SEND_BYTES;
#[cfg(test)]
pub(crate) use crate::lsp_client::{Action, Cmd, Inbound, Phase};
#[cfg(test)]
pub(crate) use wordcartel_core::diagnostics::Diagnostic;

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
// `pub`, not `pub(crate)` (a deviation from the T1 brief's literal code): the external
// `tests/harper_ls_integration.rs` integration crate names `HarperLs` (= `LspProvider<HarperEngine>`)
// and calls its `DiagnosticsProvider` methods, which requires the crate boundary to see
// `HarperEngine` too — a `pub(crate)` engine type is a hard E0599/"private type" error there,
// not just a lint, since integration tests compile as a separate crate.
#[derive(Debug)]
pub struct HarperEngine;

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
// T1 has no production consumer of this alias (only the pinned tests do); later tasks
// (warm-phase, suspend/resume) are expected to use it directly, per the T1 interfaces contract.
#[allow(dead_code)]
pub(crate) type HarperState = crate::lsp_client::ClientState<HarperEngine>;
/// The app-side harper provider handle — the pre-T1 name (external callers + tests).
pub type HarperLs = crate::lsp_client::LspProvider<HarperEngine>;
/// The harper flush guard — the pre-T1 name (test struct literals).
// Only the pinned tests construct this alias in T1; production use arrives with later tasks.
#[allow(dead_code)]
pub(crate) type FlushGuard = crate::lsp_client::FlushGuard<HarperEngine>;

impl crate::lsp_client::ClientState<HarperEngine> {
    /// Test-visible harper settings for the CURRENT cfg — the pre-T1 method, preserved for
    /// the pin (a concrete inherent impl on the monomorphized type; spec §3.1).
    // Only the pinned tests call this in T1; kept for the pin + future production use.
    #[allow(dead_code)]
    pub(crate) fn settings_object(&self) -> Value { harper_settings(&self.cfg) }
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

#[cfg(test)]
mod tests {
    use super::*;
    use wordcartel_core::diagnostics::Suggestion;

    fn cfg(grammar: bool) -> ProviderConfig {
        ProviderConfig { grammar, dictionary: None, max_file_length: 10_000, language: None }
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
            dictionary: Some("/d.txt".into()), max_file_length: 5, language: None }).settings_object();
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
        assert_eq!(p.source(), DiagSource::Harper);
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
