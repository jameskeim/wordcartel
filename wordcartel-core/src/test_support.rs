//! Shared reference model + edit vocabulary for the M7 property tests AND fuzz targets.
//! NO proptest here — it is a dev-dependency, absent under cfg(fuzzing).

/// A byte-faithful reference for TextBuffer: the same byte splice a ChangeSet performs.
pub fn model_apply(model: &mut String, at: usize, del: usize, ins: &str) {
    model.replace_range(at..at + del, ins);
}

/// One generated edit. Positions are BYTE offsets; callers snap to boundaries before applying.
#[derive(Clone, Debug)]
pub struct EditOp {
    pub at:  usize,
    pub del: usize,
    pub ins: String,
}

/// Snap a byte offset DOWN to the nearest char boundary of `s` (and clamp into `0..=s.len()`).
pub fn snap(s: &str, off: usize) -> usize {
    let off = off.min(s.len());
    (0..=off).rev().find(|&i| s.is_char_boundary(i)).unwrap_or(0)
}

/// Unicode-biased palette for generated strings (ASCII + multibyte + combining + ZWJ + emoji).
pub const UNICODE_PALETTE: &[&str] = &[
    "a", "Z", " ", "\n", "é", "中", "🙂", "\u{0301}", "\u{200d}",
];
