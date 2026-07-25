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

    // SUSPENDABLE is a `const bool` so clippy::assertions_on_constants fires on the plain
    // `assert!` below; allowing it here (density.rs precedent) — the test is a spec-table
    // assertion where readable prose beats a const block.
    #[allow(clippy::assertions_on_constants)]
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
