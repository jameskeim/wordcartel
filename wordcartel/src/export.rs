//! Pandoc export: probe, derived path, async dispatch, ExportDone reducer.
//!
//! Four formats: html (capture), docx, pdf, and tex (all writes-output).
//! Pandoc is optional — `probe_pandoc()` is cached and returns false when
//! pandoc is not installed; callers gate on it and show a status instead of
//! launching a subprocess.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// How pandoc writes its output for a given format.
pub enum ExportSink {
    /// Pandoc writes to stdout; we capture the bytes (html).
    Capture { ext: String },
    /// Pandoc writes to a temp file via `-o`; we rename it (docx, pdf).
    WritesOutput { ext: String },
}

/// The result of a successful pandoc run.
pub enum ExportResult {
    /// Pandoc wrote to stdout; these are the bytes.
    Bytes(Vec<u8>),
    /// Pandoc wrote to this temp file; rename it to the target.
    TempReady(PathBuf),
}

/// Stored on `Editor` while waiting for an `OverwriteExport` confirmation.
// PartialEq, Eq ADDED (spec §4.3): T12c asserts whole-value equality on
// Option<PendingExport>, which does not compile without them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingExport {
    pub ext: String,
    pub target: PathBuf,
    /// What the confirmed export will read (A22 D1) — carried across the confirm.
    pub scope: ExportScope,
    /// The flow's originating buffer (A22 D4-iv) — re-verified at dispatch.
    pub origin: crate::editor::BufferId,
}

/// What an export reads: the whole document, or the active buffer's marked block.
/// A FLAG, deliberately not offsets — offsets are re-read at dispatch because background
/// merges legally edit buffers while the picker is open and `Buffer::apply` remaps the
/// mark; a stored offset pair would go stale (A22 D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportScope {
    WholeDocument,
    MarkedBlock,
}

/// Why a block-scoped dispatch refused (A22 D2 / D4-iv). Maps 1:1 onto a status string.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ExportRefusal {
    /// The active buffer is not the one this flow started in.
    BufferChanged,
    /// Scope is MarkedBlock but the active buffer has no mark (collapsed, undone, cleared).
    NoMarkedBlock,
}

/// Resolve what this export reads, verifying the flow's captured context first.
/// Runs on the main thread at dispatch — the same moment the whole-document path already
/// snapshots `to_string()` — so the slice is coherent with the remapped mark.
pub(crate) fn resolve_export_input(
    editor: &crate::editor::Editor,
    scope: ExportScope,
    origin: crate::editor::BufferId,
) -> Result<String, ExportRefusal> {
    if editor.active().id != origin { return Err(ExportRefusal::BufferChanged); }
    match scope {
        ExportScope::WholeDocument => Ok(editor.active().document.buffer.to_string()),
        ExportScope::MarkedBlock => match editor.active().marked_block {
            Some(b) => Ok(editor.active().document.buffer.slice(b.start..b.end)),
            None => Err(ExportRefusal::NoMarkedBlock),
        },
    }
}

// ---------------------------------------------------------------------------
// probe_pandoc — cached via OnceLock
// ---------------------------------------------------------------------------

/// Returns true if `pandoc --version` can be spawned successfully.
/// Result is cached after the first call.
pub fn probe_pandoc() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        match std::process::Command::new("pandoc")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(_) => false,
        }
    })
}

// ---------------------------------------------------------------------------
// derived_export_path — swaps the extension beside the source file
// ---------------------------------------------------------------------------

/// Derive the export output path by replacing the source extension with `ext`.
///
/// `/a/b/notes.md` + `"html"` → `/a/b/notes.html`
pub fn derived_export_path(source: &Path, ext: &str) -> PathBuf {
    source.with_extension(ext)
}

// ---------------------------------------------------------------------------
// sink_for_ext — choose Capture vs WritesOutput based on format
// ---------------------------------------------------------------------------

fn sink_for_ext(ext: &str) -> ExportSink {
    match ext {
        "html" => ExportSink::Capture { ext: ext.to_owned() },
        _ => ExportSink::WritesOutput { ext: ext.to_owned() },
    }
}

// ---------------------------------------------------------------------------
// ExportOpts + pure argv/temp seams
// ---------------------------------------------------------------------------

/// Resolved per-dispatch export options (read from `Editor.export_cfg` by `do_export`,
/// so BOTH call sites — run_export and the OverwriteExport prompt arm — get them).
pub(crate) struct ExportOpts {
    pub typography: bool,
    pub pdf_engine: String,
}

/// Extension-preserving temp path beside `target`: `{stem}.tmp-{pid}.{ext}`.
/// The extension MUST stay visible to pandoc's `-o` format inference — the old
/// `{name}.tmp-{pid}` shape hid it, making pandoc default to HTML (the confirmed
/// docx/pdf bug; see the spec).
fn temp_path_for(target: &Path, ext: &str, pid: u32) -> PathBuf {
    let stem = target.file_stem().unwrap_or_default().to_string_lossy();
    let tmp_name = format!("{stem}.tmp-{pid}.{ext}");
    target.parent().map(|p| p.join(&tmp_name)).unwrap_or_else(|| PathBuf::from(&tmp_name))
}

/// Compose the WritesOutput invocation: the extension-preserving temp path AND the argv
/// built from THAT SAME path — one pure function, so the composition (not just the two
/// halves) is unit-testable. This is the guard against the exact bug class this effort
/// fixes: a future regression that rebuilds `tmp` differently would break the
/// composition test, not sail through green piece-tests.
fn writes_output_invocation(
    target: &Path, ext: &str, pid: u32, opts: &ExportOpts,
) -> (PathBuf, Vec<String>) {
    let tmp = temp_path_for(target, ext, pid);
    let argv = pandoc_argv(
        &ExportSink::WritesOutput { ext: ext.to_owned() },
        Some(&tmp),
        opts,
    );
    (tmp, argv)
}

/// Build the pandoc argv for one export. Pure — the testable seam. `out` is the
/// ALREADY-DERIVED temp path (None for the Capture/html sink; `pandoc_argv` never
/// constructs a path — the spec's contract holds).
fn pandoc_argv(sink: &ExportSink, out: Option<&Path>, opts: &ExportOpts) -> Vec<String> {
    let input = if opts.typography { "markdown" } else { "markdown-smart" };
    let mut argv = vec!["pandoc".to_owned(), "-f".to_owned(), input.to_owned()];
    match sink {
        ExportSink::Capture { ext } => {
            argv.push("-t".to_owned());
            argv.push(ext.clone());
        }
        ExportSink::WritesOutput { ext } => {
            if ext == "tex" {
                // Standalone + explicit format: a compilable document, no inference.
                argv.push("-s".to_owned());
                argv.push("-t".to_owned());
                argv.push("latex".to_owned());
            }
            if ext == "pdf" {
                argv.push(format!("--pdf-engine={}", opts.pdf_engine));
            }
            argv.push("-o".to_owned());
            argv.push(out.expect("WritesOutput requires an out path").to_string_lossy().into_owned());
        }
    }
    argv
}

// ---------------------------------------------------------------------------
// do_export — launch the actual pandoc subprocess (pub(crate) for app.rs)
// ---------------------------------------------------------------------------

/// Dispatch a pandoc export subprocess.  Sends `Msg::ExportDone` when done.
///
/// For `Capture` (html): captures stdout bytes.
/// For `WritesOutput` (docx, pdf, tex): writes to an extension-preserving temp
/// path (`{stem}.tmp-{pid}.{ext}`) via `-o`, then sends TempReady for the rename.
/// Builds `ExportOpts` from `editor.export_cfg` here, covering BOTH callers.
///
/// Returns `true` iff an export worker was dispatched; `false` iff a refusal fired (status
/// already set). The refusal tests assert on this return — "no worker was spawned" is
/// otherwise unprovable without racing the scheduler. Production callers discard it
/// (deliberately NOT #[must_use] — the refusal has already surfaced its status).
#[allow(clippy::too_many_arguments)] // full dispatch context in one place — as redirect_to_export
pub(crate) fn do_export(
    editor: &mut crate::editor::Editor,
    ext: &str,
    target: &Path,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    overwrite_confirmed: bool,
    scope: ExportScope,
    origin: crate::editor::BufferId,
    fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
) -> bool {
    let stdin = match resolve_export_input(editor, scope, origin) {
        Ok(s) => s,
        Err(r) => {
            let text = match r {
                ExportRefusal::NoMarkedBlock => "no marked block \u{2014} export cancelled",
                ExportRefusal::BufferChanged => "buffer changed \u{2014} export cancelled",
            };
            editor.set_status_full(crate::status::StatusKind::Warning, text.to_owned(),
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return false;
        }
    };
    let sink = sink_for_ext(ext);
    let buffer_id = editor.active().id; // == origin here, post-verification
    let target = target.to_path_buf();
    let msg_tx = msg_tx.clone();
    let opts = ExportOpts {
        typography: editor.export_cfg.typography,
        pdf_engine: editor.export_cfg.pdf_engine.clone(),
    };

    std::thread::spawn(move || {
        let result = guarded_export(|| run_pandoc(sink, &stdin, &target, &opts, &*fs));
        let _ = msg_tx.send(crate::app::Msg::ExportDone {
            buffer_id,
            target,
            result,
            overwrite_confirmed,
            scope,
        });
    });
    true
}

fn guarded_export(work: impl FnOnce() -> Result<ExportResult, crate::filter::FilterError>)
    -> Result<ExportResult, crate::filter::FilterError> {
    match crate::panicx::catch(work) {
        Ok(r) => r,
        Err(msg) => Err(crate::filter::FilterError::Panicked(msg)),
    }
}

/// The actual pandoc invocation (runs on a worker thread).
fn run_pandoc(
    sink: ExportSink, stdin: &str, target: &Path, opts: &ExportOpts, fs: &dyn crate::fsx::Fs,
) -> Result<ExportResult, crate::filter::FilterError> {
    use crate::filter::{CancelFlag, FilterError};

    let cancel = CancelFlag::new();
    let timeout = std::time::Duration::from_secs(30);
    let max_output = 64 * 1024 * 1024; // 64 MiB

    // Borrow `sink` so the WritesOutput arm can bind `ext: &String` and feed it to the
    // composition seam — a by-value `match sink` would move `ext` out, making the later
    // temp-path/argv derivation a use-after-partial-move (Codex Critical).
    match &sink {
        ExportSink::Capture { .. } => {
            let argv = pandoc_argv(&sink, None, opts);
            let bytes = crate::filter::run_subprocess(
                &argv,
                false,
                stdin.to_owned(),
                timeout,
                max_output,
                &cancel,
            )?;
            Ok(ExportResult::Bytes(bytes))
        }
        ExportSink::WritesOutput { ext } => {
            // The temp path fed to `-o` and the temp path checked/renamed are the SAME
            // by construction — the composition seam builds both together.
            let (tmp, argv) = writes_output_invocation(target, ext, std::process::id(), opts);
            // pandoc reads the markdown from stdin (`-f markdown…`) and writes the output
            // file itself (`-o <tmp>`); it exits 0 on success.
            crate::filter::run_subprocess(
                &argv,
                false,
                stdin.to_owned(),
                timeout,
                max_output,
                &cancel,
            )?;

            // Verify the file was written.
            if !crate::fsx::exists_via(fs, &tmp) {
                return Err(FilterError::ExportWrite(
                    format!("pandoc did not write {}", tmp.display())
                ));
            }
            Ok(ExportResult::TempReady(tmp))
        }
    }
}

// ---------------------------------------------------------------------------
// run_export — public entry point called from registry commands
// ---------------------------------------------------------------------------

/// Top-level export entry: gate on pandoc, then open a destination picker PRE-SEEDED with
/// the derived path.
///
/// The seeding is the whole point (decision 4): export is zero-decision today, and a bare
/// Enter must reproduce that byte-for-byte. Destination CHOICE is new capability;
/// destination OBLIGATION would be a regression.
pub fn run_export(
    editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    ext: &str,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    run_export_with_probe(editor, fs, ext, msg_tx, probe_pandoc)
}

/// `run_export` with an INJECTABLE pandoc probe.
///
/// The probe seam exists because the merge gate runs on machines without pandoc: a test
/// that depends on the host having it is an environment assumption that fails the gate
/// rather than the code. Production passes `probe_pandoc` (still `OnceLock`-cached);
/// tests pass a closure.
pub(crate) fn run_export_with_probe(
    editor: &mut crate::editor::Editor,
    fs: &std::sync::Arc<dyn crate::fsx::Fs + Send + Sync>,
    ext: &str,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    pandoc_available: impl Fn() -> bool,
) {
    // Both refusals stay AHEAD of the picker — no point choosing a destination for an
    // export that cannot run.
    let source = match editor.active().document.path.clone() {
        Some(p) => p,
        None => {
            editor.set_status_full(crate::status::StatusKind::Warning, "save the file first before exporting",
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return;
        }
    };
    if !pandoc_available() {
        editor.set_status_full(crate::status::StatusKind::Error, "pandoc not found — install it to export",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return;
    }

    // `derived_export_path` still computes the default — it is now the SEED rather than the
    // final answer, and it reads `Document.path`, which stays LOGICAL (§7.6.2), so the
    // output lands beside the file the writer opened.
    let derived = derived_export_path(&source, ext);
    let dir = derived.parent().map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let field = derived.file_name().map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    editor.open_destination_picker(fs, msg_tx,
        crate::file_browser::DestinationPurpose::Export { ext: ext.to_owned(),
            scope: ExportScope::WholeDocument, origin: editor.active().id }, dir, field);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_export_path_swaps_extension_beside_source() {
        let p = derived_export_path(std::path::Path::new("/a/b/notes.md"), "html");
        assert_eq!(p, std::path::Path::new("/a/b/notes.html"));
    }

    #[test]
    fn export_refuses_scratch_buffer() {
        use crate::editor::Editor;
        let mut e = Editor::new_from_text("x\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        run_export(&mut e, &crate::test_support::test_fs(), "html", &tx);
        assert!(e.status_text().to_lowercase().contains("save the file first"));
        // A17 T5 (F4 Warning table): a Sticky Warning.
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
    }

    #[test]
    fn export_opens_a_destination_picker_pre_seeded_with_the_derived_path() {
        // ENTER-THROUGH (decision 4). Export is zero-decision today; adding a mandatory
        // dialog would be a regression dressed as a feature. Pre-seeding means a bare Enter
        // reproduces today's behaviour byte-for-byte, with the target VISIBLE while doing so.
        let d = std::env::temp_dir().join(format!("wc-exp-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        let src = d.join("notes.md");
        std::fs::write(&src, b"# hi\n").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("# hi\n", Some(src.clone()), (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();

        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        run_export_with_probe(&mut e, &fs, "html", &tx, || true);

        let fb = e.file_browser.as_ref().expect("export opens the destination picker");
        assert_eq!(fb.dir, d, "seeded at the SOURCE's directory");
        match &fb.mode {
            crate::file_browser::BrowseMode::Destination { purpose, field, .. } => {
                // Compare BY REFERENCE — `DestinationPurpose::Export { ext: String }` is not
                // `Copy`, so `*purpose` would move a `String` out of a borrow of `fb.mode`.
                assert_eq!(purpose, &crate::file_browser::DestinationPurpose::Export {
                    ext: "html".into(), scope: ExportScope::WholeDocument, origin: e.active().id },
                    "a MarkedBlock-defaulting run_export breaks the palette invariant");
                assert_eq!(field, "notes.html",
                    "pre-filled with derived_export_path's file name, so bare Enter == today");
            }
            other => panic!("expected a destination picker, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn export_destination_picker_opens_without_pandoc_installed() {
        // The merge gate runs on machines with no pandoc. `run_export` probes
        // `pandoc --version` before anything else, so an environment assumption here would
        // fail the gate rather than the code. The probe is injected, not detected.
        let d = std::env::temp_dir().join(format!("wc-exp-nopandoc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("dir");
        let src = d.join("notes.md");
        std::fs::write(&src, b"# hi\n").expect("seed");
        let mut e = crate::editor::Editor::new_from_text("# hi\n", Some(src), (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        // Pandoc PRESENT (injected) → the picker opens regardless of the host machine.
        run_export_with_probe(&mut e, &fs, "html", &tx, || true);
        assert!(e.file_browser.is_some(), "an injected-present probe opens the picker");
        // Pandoc ABSENT (injected) → the refusal fires and no picker opens.
        e.file_browser = None;
        run_export_with_probe(&mut e, &fs, "html", &tx, || false);
        assert!(e.file_browser.is_none(), "an injected-absent probe opens NO picker");
        assert!(e.status_text().to_lowercase().contains("pandoc not found"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn export_still_refuses_before_opening_any_picker() {
        // The probe and the unnamed-buffer refusal stay AHEAD of the picker: there is no
        // point choosing a destination for an export that cannot run.
        let mut e = crate::editor::Editor::new_from_text("x\n", None, (80, 24));
        let (tx, _rx) = std::sync::mpsc::channel();
        let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =
            std::sync::Arc::new(crate::fsx::RealFs);
        run_export_with_probe(&mut e, &fs, "html", &tx, || true);
        assert!(e.file_browser.is_none(), "an unnamed buffer opens NO picker");
        assert!(e.status_text().to_lowercase().contains("save the file first"));
    }

    // T1 — the pure seam. Each case flips exactly ONE conjunct of
    // `scope carried ∧ origin unchanged ∧ mark present` (spec §11 E11 rule).
    #[test]
    fn resolve_export_input_whole_document_reads_everything() {
        let e = crate::editor::Editor::new_from_text("AAA\nBBB\nCCC\n", None, (80, 24));
        let id = e.active().id;
        assert_eq!(resolve_export_input(&e, ExportScope::WholeDocument, id),
            Ok("AAA\nBBB\nCCC\n".to_owned()));
    }

    #[test]
    fn resolve_export_input_marked_block_reads_only_the_block() {
        let mut e = crate::editor::Editor::new_from_text("AAA\nBBB\nCCC\n", None, (80, 24));
        let id = e.active().id;
        // hidden: true on purpose — pins the "hidden is display-only, ignored by content
        // reads" call (spec §2): an implementation consulting `hidden` skips the slice.
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 4, end: 8, hidden: true });
        assert_eq!(resolve_export_input(&e, ExportScope::MarkedBlock, id),
            Ok("BBB\n".to_owned()),
            "the scope-dropped implementation returns the whole text here");
    }

    #[test]
    fn resolve_export_input_refuses_when_the_mark_is_gone() {
        let e = crate::editor::Editor::new_from_text("AAA\nBBB\n", None, (80, 24));
        let id = e.active().id;
        assert_eq!(resolve_export_input(&e, ExportScope::MarkedBlock, id),
            Err(ExportRefusal::NoMarkedBlock),
            "a whole-document-fallback implementation returns Ok(whole) — D2 forbids it");
    }

    #[test]
    fn resolve_export_input_refuses_when_the_buffer_changed() {
        let mut e = crate::editor::Editor::new_from_text("AAA\nBBB\n", None, (80, 24));
        let a = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false });
        crate::workspace::new_empty_buffer(&mut e); // appends AND switches active
        assert_ne!(e.active().id, a, "fixture: the active buffer really changed");
        assert_eq!(resolve_export_input(&e, ExportScope::MarkedBlock, a),
            Err(ExportRefusal::BufferChanged),
            "an origin-ignoring implementation reads the NEW active buffer and refuses \
             with NoMarkedBlock (or slices the wrong buffer) instead");
    }

    // A22 ORDERINGS (adopted from the whole-branch gate) — the degenerate mark at dispatch.
    // A collapsed, zero-width mark satisfies "mark present", so the seam must slice it rather
    // than refuse or panic: an implementation that asserted `start < end`, or that treated an
    // empty slice as "no block", turns a harmless no-op export into a crash or a wrong
    // refusal. Empty input is pandoc's problem, not the seam's.
    #[test]
    fn resolve_export_input_slices_a_collapsed_mark_to_the_empty_string() {
        let mut e = crate::editor::Editor::new_from_text("AAA\n", None, (80, 24));
        let id = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 2, end: 2, hidden: false });
        assert_eq!(resolve_export_input(&e, ExportScope::MarkedBlock, id), Ok(String::new()),
            "a whole-document fallback returns Ok(\"AAA\\n\") and a start<end assert panics");
    }

    // T5 — dispatch refusals. The bool return is the scheduling-free discriminator: a
    // wrongly-dispatching implementation returns true and fails HERE, before any race.
    #[test]
    fn do_export_refuses_block_scope_with_no_mark_and_dispatches_nothing() {
        let mut e = crate::editor::Editor::new_from_text("# hi\n", None, (80, 24));
        let id = e.active().id;
        let (tx, rx) = std::sync::mpsc::channel();
        let d = crate::test_support::scratch_dir("a22-t5-refuse");
        let target = d.join("out.html");
        let dispatched = do_export(&mut e, "html", &target, &tx, false,
            ExportScope::MarkedBlock, id, crate::test_support::test_fs());
        assert!(!dispatched, "a wrongly-dispatching implementation returns true");
        assert_eq!(e.status_text(), "no marked block — export cancelled");
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
        // Deterministic absence, no timeout: drop OUR sender; a spawned worker would hold
        // a clone and its guaranteed single send would arrive as Ok — a wrong
        // implementation fails this recv deterministically once its worker finishes,
        // and a correct one errs Disconnected immediately.
        drop(tx);
        assert!(rx.recv().is_err(), "no worker may exist to ever send ExportDone");
    }

    #[test]
    fn do_export_refuses_when_the_buffer_changed() {
        let mut e = crate::editor::Editor::new_from_text("AAA\nBBB\n", None, (80, 24));
        let a = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false });
        crate::workspace::new_empty_buffer(&mut e);
        let (tx, rx) = std::sync::mpsc::channel();
        let d = crate::test_support::scratch_dir("a22-t5-switched");
        let dispatched = do_export(&mut e, "html", &d.join("out.html"), &tx, false,
            ExportScope::MarkedBlock, a, crate::test_support::test_fs());
        // B (the fresh "\n" buffer) has no mark, so an origin-re-deriving or verify-skipping
        // implementation ALSO refuses here — but with "no marked block — export cancelled".
        // The bool alone does not discriminate in this fixture; the STATUS EQUALITY does.
        assert!(!dispatched);
        assert_eq!(e.status_text(), "buffer changed — export cancelled",
            "a re-deriving/verify-skipping implementation reads 'no marked block — export \
             cancelled' here");
        drop(tx);
        assert!(rx.recv().is_err());
    }

    // T5 positive control + T13a — proves `false`/no-message is a discriminating reading,
    // and pins the do_export → Msg::ExportDone scope hand-off AT BOTH VALUES: a message
    // construction hardcoding EITHER scope fails the opposite iteration (a carriage claim
    // tested at one value is passed by an implementation that hardcodes that value).
    #[test]
    fn do_export_dispatches_and_the_message_carries_the_scope() {
        for scope in [ExportScope::MarkedBlock, ExportScope::WholeDocument] {
            let mut e = crate::editor::Editor::new_from_text("AAA\nBBB\n", None, (80, 24));
            let id = e.active().id;
            if scope == ExportScope::MarkedBlock {
                e.active_mut().marked_block =
                    Some(crate::editor::MarkedBlock { start: 0, end: 4, hidden: false });
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let d = crate::test_support::scratch_dir("a22-t5-control");
            let dispatched = do_export(&mut e, "html", &d.join("out.html"), &tx, false,
                scope, id, crate::test_support::test_fs());
            assert!(dispatched, "{scope:?}: conjuncts all hold, must dispatch");
            // BOUNDED receive (the file_browser_commit/e2e precedent): guarded_export
            // guarantees a spawned worker sends exactly one ExportDone, pandoc or no pandoc.
            let msg = rx.recv_timeout(std::time::Duration::from_secs(10))
                .expect("the worker always sends exactly one ExportDone");
            match msg {
                crate::app::Msg::ExportDone { scope: got, .. } => assert_eq!(got, scope,
                    "T13a: a construction hardcoding the other scope fails this iteration"),
                other => panic!("expected ExportDone, got {other:?}"),
            }
        }
    }

    // T11 — the reason scope is a FLAG: the read follows the funnel's remap.
    #[test]
    fn resolve_export_input_reads_the_remapped_mark_not_stored_offsets() {
        let mut e = crate::editor::Editor::new_from_text("hello world\n", None, (40, 10));
        let id = e.active().id;
        e.active_mut().marked_block =
            Some(crate::editor::MarkedBlock { start: 6, end: 11, hidden: false }); // "world"
        let doc_len = e.active().document.buffer.len();
        let (cs, edit) = crate::commands::build_multi_replace(&[(0, 0, "pre ".into())], doc_len);
        let txn = wordcartel_core::history::Transaction::new(cs)
            .with_selection(wordcartel_core::selection::Selection::single(0));
        let _ = e.apply(txn, edit, wordcartel_core::history::EditKind::Other,
            &crate::test_support::TestClock(0));
        assert_eq!(resolve_export_input(&e, ExportScope::MarkedBlock, id),
            Ok("world".to_owned()),
            "a stored-offsets design still slices [6,11) of 'pre hello world\\n' = 'llo w'");
    }

    #[test]
    fn guarded_export_maps_panic_to_err() {
        let r = guarded_export(|| panic!("exp"));
        assert!(matches!(r, Err(crate::filter::FilterError::Panicked(ref m)) if m == "exp"));
    }

    fn opts(typo: bool, engine: &str) -> ExportOpts {
        ExportOpts { typography: typo, pdf_engine: engine.into() }
    }

    #[test]
    fn argv_html_matches_today_when_typography_on() {
        let a = pandoc_argv(&ExportSink::Capture { ext: "html".into() }, None, &opts(true, "xelatex"));
        assert_eq!(a, vec!["pandoc", "-f", "markdown", "-t", "html"]);
    }
    #[test]
    fn argv_typography_off_uses_markdown_smart_minus() {
        let a = pandoc_argv(&ExportSink::Capture { ext: "html".into() }, None, &opts(false, "xelatex"));
        assert_eq!(a, vec!["pandoc", "-f", "markdown-smart", "-t", "html"]);
    }
    #[test]
    fn argv_docx_gets_extension_preserving_out_path() {
        let out = std::path::Path::new("/a/notes.tmp-123.docx");
        let a = pandoc_argv(&ExportSink::WritesOutput { ext: "docx".into() }, Some(out), &opts(true, "xelatex"));
        assert_eq!(a, vec!["pandoc", "-f", "markdown", "-o", "/a/notes.tmp-123.docx"]);
    }
    #[test]
    fn argv_pdf_carries_the_engine_flag() {
        let out = std::path::Path::new("/a/notes.tmp-123.pdf");
        let a = pandoc_argv(&ExportSink::WritesOutput { ext: "pdf".into() }, Some(out), &opts(true, "tectonic"));
        assert_eq!(a, vec!["pandoc", "-f", "markdown", "--pdf-engine=tectonic", "-o", "/a/notes.tmp-123.pdf"]);
    }
    #[test]
    fn argv_tex_is_standalone_explicit_latex() {
        let out = std::path::Path::new("/a/notes.tmp-123.tex");
        let a = pandoc_argv(&ExportSink::WritesOutput { ext: "tex".into() }, Some(out), &opts(true, "xelatex"));
        assert_eq!(a, vec!["pandoc", "-f", "markdown", "-s", "-t", "latex", "-o", "/a/notes.tmp-123.tex"]);
    }
    #[test]
    fn temp_path_preserves_the_format_extension() {
        let t = temp_path_for(std::path::Path::new("/a/b/notes.pdf"), "pdf", 123);
        assert_eq!(t, std::path::Path::new("/a/b/notes.tmp-123.pdf"));
    }
    #[test]
    fn writes_output_invocation_composes_tmp_and_argv_coherently() {
        // The composition guard (Fable I-1): the argv's -o element IS the returned tmp,
        // and the tmp carries the format extension — a regression that rebuilds either
        // half differently fails HERE even if the piece-tests stay green.
        let (tmp, argv) =
            writes_output_invocation(std::path::Path::new("/a/notes.pdf"), "pdf", 123, &opts(true, "xelatex"));
        let o_pos = argv.iter().position(|a| a == "-o").expect("-o present");
        assert_eq!(argv[o_pos + 1], tmp.to_string_lossy(), "argv -o must be the returned tmp");
        assert!(tmp.extension().is_some_and(|e| e == "pdf"), "tmp must end with the format ext: {tmp:?}");
    }
}
