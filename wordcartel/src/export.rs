//! Pandoc export: probe, derived path, async dispatch, ExportDone reducer.
//!
//! Three presets: html (capture), docx (writes output), pdf (writes output).
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
// do_export — launch the actual pandoc subprocess (pub(crate) for app.rs)
// ---------------------------------------------------------------------------

/// Dispatch a pandoc export subprocess.  Sends `Msg::ExportDone` when done.
///
/// For `Capture` (html): passes `-t html`, captures stdout bytes.
/// For `WritesOutput` (docx, pdf): passes `-o <target>.tmp-<pid>`, sends TempReady.
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

    std::thread::spawn(move || {
        let result = run_pandoc(sink, &stdin, &target);
        let _ = msg_tx.send(crate::app::Msg::ExportDone {
            buffer_id,
            target,
            result,
            overwrite_confirmed,
        });
    });
}

/// The actual pandoc invocation (runs on a worker thread).
fn run_pandoc(sink: ExportSink, stdin: &str, target: &Path) -> Result<ExportResult, crate::filter::FilterError> {
    use crate::filter::{CancelFlag, FilterError};

    let cancel = CancelFlag::new();
    let timeout = std::time::Duration::from_secs(30);
    let max_output = 64 * 1024 * 1024; // 64 MiB

    match sink {
        ExportSink::Capture { .. } => {
            let argv = vec![
                "pandoc".to_owned(),
                "-f".to_owned(),
                "markdown".to_owned(),
                "-t".to_owned(),
                "html".to_owned(),
            ];
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
            // Build a unique temp output path in the same directory as target.
            let pid = std::process::id();
            let tmp_name = format!(
                "{}.tmp-{}",
                target.file_name().unwrap_or_default().to_string_lossy(),
                pid
            );
            let tmp = target.parent().map(|p| p.join(&tmp_name))
                .unwrap_or_else(|| PathBuf::from(&tmp_name));

            let argv = vec![
                "pandoc".to_owned(),
                "-f".to_owned(),
                "markdown".to_owned(),
                "-o".to_owned(),
                tmp.to_string_lossy().into_owned(),
            ];
            // For WritesOutput formats, pandoc writes to the file; stdin is the markdown.
            // We pass the content via stdin by using `-f markdown` and reading from stdin
            // by not providing a file argument. However, pandoc needs an input.
            // We must provide stdin content and let pandoc read from stdin.
            crate::filter::run_subprocess(
                &argv,
                false,
                stdin.to_owned(),
                timeout,
                max_output,
                &cancel,
            ).map_err(|e| e)?; // child exits 0 on success

            // Verify the file was written.
            if !tmp.exists() {
                return Err(FilterError::ExportWrite(
                    format!("pandoc did not write {}", tmp.display())
                ));
            }
            let _ = ext; // silence unused warning
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
            editor.status = "save the file first before exporting".into();
            return;
        }
    };

    // Pandoc availability gate.
    if !probe_pandoc() {
        editor.status = "pandoc not found — install it to export".into();
        return;
    }

    let target = derived_export_path(&source, ext);

    // If target exists, ask for overwrite confirmation.
    if target.exists() {
        editor.pending_export = Some(PendingExport {
            ext: ext.to_owned(),
            target: target.clone(),
        });
        editor.prompt = Some(crate::prompt::Prompt::export_overwrite(&target));
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
        assert!(e.status.to_lowercase().contains("save the file first"));
    }
}
