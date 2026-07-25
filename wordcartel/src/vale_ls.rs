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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp_client::LspEngine;
    use crate::diag_provider::ProviderConfig;
    use serde_json::json;

    fn cfg() -> ProviderConfig {
        ProviderConfig { grammar: true, dictionary: None, max_file_length: 10_000, language: None }
    }

    // SUSPENDABLE is a `const bool` so clippy::assertions_on_constants fires on the plain
    // `assert!` below; allowing it here (ltex_ls.rs precedent) — the test is a spec-table
    // assertion where readable prose beats a const block.
    #[allow(clippy::assertions_on_constants)]
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
