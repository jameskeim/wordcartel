//! Filter primitive — data types only (no execution).
//!
//! Execution (spawning subprocesses, merging output) is wired in Task 2/3.
//! Export types (`ExportSink`, `ExportResult`) live in `export.rs` (Task 5).

// wired in Task 2/3/5
#[allow(dead_code)]
/// Specification for a text-filter command.
pub struct FilterSpec {
    pub argv: Vec<String>,
    pub shell: bool,
    pub disposition: Disposition,
    pub input: Input,
    pub timeout: std::time::Duration,
    pub max_output: usize,
}

// wired in Task 2/3
#[allow(dead_code)]
/// How the filter's output is merged back into the document.
///
/// TEXT-ONLY: `Export` is a separate path built in Task 5 — not here.
#[derive(Clone, Debug)]
pub enum Disposition {
    /// Replace the input range with the filter's stdout.
    Filter,
    /// Insert the filter's stdout after the input range (input is not deleted).
    Insert,
}

// wired in Task 2/3
#[allow(dead_code)]
/// What bytes are sent to the subprocess's stdin.
#[derive(Clone, Debug)]
pub enum Input {
    /// Send the primary selection if non-empty, otherwise the whole buffer.
    SelectionElseBuffer,
    /// Send nothing (empty stdin).
    None,
    /// Always send the entire buffer.
    WholeBuffer,
}

// wired in Task 2/3
#[allow(dead_code)]
/// Errors that can occur when running a filter.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterError {
    /// The subprocess could not be spawned.
    Spawn(String),
    /// The subprocess exited with a non-zero status.
    NonZero { code: String, stderr: String },
    /// The subprocess did not finish within the timeout.
    Timeout,
    /// The operation was cancelled.
    Cancelled,
    /// The subprocess output exceeded `max_output` bytes.
    TooLarge,
    /// The subprocess output was not valid UTF-8.
    NotUtf8,
    /// Writing the export file failed (used by export path, Task 5).
    ExportWrite(String),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_spec_constructs() {
        let s = FilterSpec {
            argv: vec!["cat".into()],
            shell: false,
            disposition: Disposition::Filter,
            input: Input::SelectionElseBuffer,
            timeout: std::time::Duration::from_secs(10),
            max_output: 1 << 20,
        };
        assert_eq!(s.argv, vec!["cat".to_string()]);
        assert!(matches!(s.disposition, Disposition::Filter));
    }
}
