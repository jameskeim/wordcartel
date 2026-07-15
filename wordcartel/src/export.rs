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
#[derive(Debug, Clone)]
pub struct PendingExport {
    pub ext: String,
    pub target: PathBuf,
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
pub(crate) fn do_export(
    editor: &mut crate::editor::Editor,
    ext: &str,
    target: &Path,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
    overwrite_confirmed: bool,
) {
    let sink = sink_for_ext(ext);
    let buffer_id = editor.active().id;
    let stdin = editor.active().document.buffer.to_string();
    let target = target.to_path_buf();
    let msg_tx = msg_tx.clone();
    let opts = ExportOpts {
        typography: editor.export_cfg.typography,
        pdf_engine: editor.export_cfg.pdf_engine.clone(),
    };

    std::thread::spawn(move || {
        let result = guarded_export(|| run_pandoc(sink, &stdin, &target, &opts));
        let _ = msg_tx.send(crate::app::Msg::ExportDone {
            buffer_id,
            target,
            result,
            overwrite_confirmed,
        });
    });
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
    sink: ExportSink, stdin: &str, target: &Path, opts: &ExportOpts,
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
            if !tmp.exists() {
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

/// Top-level export entry: gate on pandoc probe, derive target path, handle
/// overwrite confirmation, then dispatch.
pub fn run_export(
    editor: &mut crate::editor::Editor,
    ext: &str,
    msg_tx: &std::sync::mpsc::Sender<crate::app::Msg>,
) {
    // Must have a named file (not a scratch buffer).
    let source = match editor.active().document.path.clone() {
        Some(p) => p,
        None => {
            editor.set_status_full(crate::status::StatusKind::Warning, "save the file first before exporting",
                crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
            return;
        }
    };

    // Pandoc availability gate.
    if !probe_pandoc() {
        editor.set_status_full(crate::status::StatusKind::Error, "pandoc not found — install it to export",
            crate::status::StatusLifetime::Sticky, crate::status::StatusSource::Host, None);
        return;
    }

    let target = derived_export_path(&source, ext);

    // If target exists, ask for overwrite confirmation.
    if target.exists() {
        editor.pending_export = Some(PendingExport {
            ext: ext.to_owned(),
            target: target.clone(),
        });
        editor.open_prompt(crate::prompt::Prompt::export_overwrite(&target));
        return;
    }

    // Target did not exist: dispatch without overwrite confirmation.  If it
    // appears before the write completes, finalization refuses to clobber it.
    do_export(editor, ext, &target, msg_tx, false);
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
        run_export(&mut e, "html", &tx);
        assert!(e.status_text().to_lowercase().contains("save the file first"));
        // A17 T5 (F4 Warning table): a Sticky Warning.
        assert_eq!(e.status().unwrap().kind(), crate::status::StatusKind::Warning);
        assert_eq!(e.status().unwrap().lifetime(), crate::status::StatusLifetime::Sticky);
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
