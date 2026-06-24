//! In-process repar transforms (Reflow / Unwrap / Ventilate). The typed wrapper
//! `run_transform` is the ONLY place that touches repar's stringly public API.

pub const DEFAULT_REFLOW_WIDTH: u32 = 72;
/// Regions at or above this byte length run off the keystroke thread (§5.2).
#[allow(dead_code)] // wired in Task 3/4
pub const TRANSFORM_ASYNC_THRESHOLD: usize = 1 << 20; // 1 MiB

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransformKind { Reflow, Unwrap, Ventilate }

impl TransformKind {
    fn verb(self) -> &'static str {
        match self {
            TransformKind::Reflow    => "--reflow",
            TransformKind::Unwrap    => "--unwrap",
            TransformKind::Ventilate => "--ventilate",
        }
    }
    /// Past-tense success word: "reflowed" / "unwrapped" / "ventilated".
    #[allow(dead_code)] // wired in Task 3/4
    pub fn past_tense(self) -> &'static str {
        match self { Self::Reflow => "reflowed", Self::Unwrap => "unwrapped", Self::Ventilate => "ventilated" }
    }
    /// Gerund for in-progress: "reflowing" / "unwrapping" / "ventilating".
    #[allow(dead_code)] // wired in Task 3/4
    pub fn gerund(self) -> &'static str {
        match self { Self::Reflow => "reflowing", Self::Unwrap => "unwrapping", Self::Ventilate => "ventilating" }
    }
}

#[allow(dead_code)] // wired in Task 3/4
#[derive(Debug)]
pub enum TransformError { Repar(String) }

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self { TransformError::Repar(m) => write!(f, "{m}") }
    }
}

impl TransformError {
    #[allow(dead_code)] // wired in Task 3/4
    fn from_repar(e: repar::ParError) -> TransformError { TransformError::Repar(e.to_string()) }
}

/// Run a repar transform over `input`, markdown-aware. Pure (no IO).
pub fn run_transform(kind: TransformKind, input: &str, width: u32) -> Result<String, TransformError> {
    let mut opts = repar::Options::new().width(width);
    // apply_par_args takes &mut self and returns PResult<()> — not chainable.
    opts.apply_par_args([kind.verb()]).map_err(TransformError::from_repar)?;
    opts.apply_fixups("markdown").map_err(TransformError::from_repar)?; // Compat::MARKDOWN
    opts.format(input).map_err(TransformError::from_repar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reflow_wraps_long_prose_within_width() {
        let long = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega";
        let out = run_transform(TransformKind::Reflow, long, 72).unwrap();
        for line in out.lines() {
            // repar::display_width(s, start_col, tab, compat) — 4 args (width.rs).
            let cols = repar::display_width(line, 0, 8, repar::Compat::empty());
            assert!(cols <= 72, "line over width ({cols}): {line:?}");
        }
        // Round-trip back to words: unwrapping the reflow yields one line with the same words.
        let unwrapped = run_transform(TransformKind::Unwrap, &out, 72).unwrap();
        assert_eq!(unwrapped.split_whitespace().collect::<Vec<_>>(),
                   long.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn unwrap_joins_a_wrapped_paragraph_to_one_logical_line() {
        let wrapped = "one two three\nfour five six\nseven eight\n";
        let out = run_transform(TransformKind::Unwrap, wrapped, 72).unwrap();
        // One paragraph → one non-empty logical line.
        assert_eq!(out.lines().filter(|l| !l.trim().is_empty()).count(), 1);
        assert_eq!(out.split_whitespace().collect::<Vec<_>>(),
                   wrapped.split_whitespace().collect::<Vec<_>>());
    }

    #[test]
    fn ventilate_breaks_one_sentence_per_line() {
        let para = "First sentence here. Second sentence here. Third one here.\n";
        let out = run_transform(TransformKind::Ventilate, para, 72).unwrap();
        assert_eq!(out.lines().filter(|l| !l.trim().is_empty()).count(), 3);
    }

    #[test]
    fn markdown_mode_passes_fenced_code_through_verbatim() {
        // A long line INSIDE a fenced code block must NOT be reflowed/wrapped.
        let long_code = "let x = aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa;";
        let input = format!("```\n{long_code}\n```\n");
        let out = run_transform(TransformKind::Reflow, &input, 72).unwrap();
        assert!(out.contains(long_code), "fenced code line must survive verbatim:\n{out}");
    }

    #[test]
    fn markdown_mode_leaves_heading_unwrapped() {
        let input = "# A heading that is fairly long but is a heading not prose\n\nbody text\n";
        let out = run_transform(TransformKind::Reflow, &input, 72).unwrap();
        assert!(out.contains("# A heading that is fairly long but is a heading not prose"),
                "heading must pass through:\n{out}");
    }
}
