//! Real-binary integration test for the `HarperLs` provider (Effort A, T6). `#[ignore]`-gated;
//! skips cleanly when `harper-ls` is not on PATH. Unlike `harper_ls_probe.rs` (which drives raw
//! JSON-RPC stdio to reconfirm two protocol facts), this drives the actual production seam — the
//! `DiagnosticsProvider` impl on `HarperLs` — through `ensure_running`/`configure`/`notify_change`,
//! pumping the real `Sender<Msg>`/`Receiver<Msg>` the shell uses. This exercises the config-pull
//! responder end to end (without which harper-ls emits nothing — spec §8) and the eager-assemble
//! codeAction path that attaches `Suggestion::ReplaceWith`.
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use wordcartel::app::Msg;
use wordcartel::diag_provider::{Accepted, DiagnosticsProvider, ProviderConfig};
use wordcartel::editor::BufferId;
use wordcartel::harper_ls::HarperLs;
use wordcartel_core::diagnostics::{Diagnostic, DiagnosticKind, DiagSource, Suggestion};

fn harper_on_path() -> bool {
    Command::new("harper-ls").arg("--version").stdout(Stdio::null())
        .stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// Pump `rx` up to `timeout`, returning the diagnostics of the first terminal `DiagnosticsDone`
/// for `(want_buffer, want_version)`. Non-matching messages (e.g. a startup `DiagProviderEvent`)
/// are drained and ignored — the provider's real client thread emits those too.
fn await_diagnostics_done(
    rx: &mpsc::Receiver<Msg>,
    want_buffer: BufferId,
    want_version: u64,
    timeout: Duration,
) -> Option<Vec<Diagnostic>> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return None; }
        match rx.recv_timeout(remaining) {
            Ok(Msg::DiagnosticsDone { buffer_id, version, source, diagnostics })
                if buffer_id == want_buffer && version == want_version => {
                assert_eq!(source, DiagSource::Harper, "the real harper-ls provider tags Harper");
                return Some(diagnostics);
            }
            Ok(_) => continue, // e.g. DiagProviderEvent(Restarted) while the child spawns
            Err(RecvTimeoutError::Timeout) => return None,
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

#[test]
#[ignore = "requires harper-ls on PATH; run with --ignored"]
// Skip-diagnostic prints to stderr; the workspace denies clippy::print_stderr, so allow it here
// (item-local, house-style exception — an ignored integration test's skip message is legitimate).
#[allow(clippy::print_stderr)]
fn provider_flags_a_misspelling_and_respects_the_dictionary() {
    if !harper_on_path() { eprintln!("skip: harper-ls not on PATH"); return; }

    let dir = std::env::temp_dir().join(format!("wcartel_harper_it_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir temp dir");
    let dict_path = dir.join("dictionary.txt");
    std::fs::write(&dict_path, "wcartelword\n").expect("write dictionary");

    let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
    let cfg = ProviderConfig {
        grammar: true,
        dictionary: Some(dict_path.clone()),
        max_file_length: 10_000_000,
    };
    let mut provider = HarperLs::new(msg_tx, cfg.clone());
    provider.ensure_running();
    provider.configure(cfg); // resends the (identical) config; exercises the seam explicitly

    let buffer_id = BufferId(1);
    // "splelling" is a genuine misspelling (harper-ls 2.1.0 classifies it `code: "SpellCheck"`,
    // our client-side `classify_lsp` → `DiagnosticKind::Spelling`) — deliberately NOT "teh", which
    // harper-ls tags with its own dedicated `code: "The"` typo-fix linter and so classifies as
    // `Grammar` under our code/source/message heuristic (empirically reconfirmed against the
    // packaged binary while writing this test).
    let text = "wcartelword splelling mistake\n";
    let accepted = provider.notify_change(buffer_id, 1, None, text.to_string());
    assert_eq!(accepted, Accepted::Yes, "notify_change must be accepted by a live client thread");

    let diagnostics = await_diagnostics_done(&msg_rx, buffer_id, 1, Duration::from_secs(30))
        .expect("a terminal DiagnosticsDone for (buffer_id=1, version=1) within 30s");

    provider.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    let misspelling = diagnostics.iter()
        .find(|d| d.kind == DiagnosticKind::Spelling && text.get(d.range.clone()) == Some("splelling"))
        .unwrap_or_else(|| panic!("expected a Spelling diagnostic for 'splelling', got {diagnostics:?}"));
    assert!(
        misspelling.suggestions.iter().any(|s| matches!(s, Suggestion::ReplaceWith(t) if t == "spelling")),
        "expected ReplaceWith(\"spelling\") among 'splelling' suggestions, got {:?}", misspelling.suggestions
    );
    assert!(
        diagnostics.iter().all(|d| text.get(d.range.clone()) != Some("wcartelword")),
        "the dictionary word ('wcartelword') must not be flagged, got {diagnostics:?}"
    );
}
