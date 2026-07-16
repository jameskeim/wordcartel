//! INV-SEAM (H22, J3): after the universal-edit-chokepoint migration, `Buffer::apply` is the sole
//! raw mutation channel. Two guards: (1) `Buffer::apply` is `pub(crate)` — the COMPILER blocks
//! every out-of-crate bypass (the Effort-P concern). (2) This heuristic source scan catches the
//! COMMON in-crate regression: a raw `Buffer::apply` reached through an accessor CHAIN
//! (`active_mut()`/`active()`/`by_id_mut(..)` immediately `.apply(`) anywhere in production.
//! The sanctioned core writes it as a two-statement `let b = by_id_mut(..); b.apply(..)` pair, so
//! it is NOT matched and needs no allowlist. Residual (documented, spec §8.1): a `let`-bound
//! Buffer local elsewhere would evade the text scan — the `pub(crate)` compiler guard + review
//! cover that. Heuristic by design, paired with the compiler.
use std::path::{Path, PathBuf};

/// Production region = source before a co-located `mod tests` (mirrors module_budgets.rs:21).
fn production(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    match lines.iter().rposition(|l| l.trim_start().starts_with("mod tests")) {
        Some(i) => lines[..i].join("\n"),
        None => src.to_string(),
    }
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() { rs_files(&p, out); }
        else if p.extension().and_then(|x| x.to_str()) == Some("rs") { out.push(p); }
    }
}

/// A line reaches a raw `Buffer::apply` through an accessor chain.
fn is_chained_raw_apply(line: &str) -> bool {
    line.contains(".active_mut().apply(")
        || line.contains(".active().apply(")
        || (line.contains("by_id_mut(") && line.contains(".apply("))
}

#[test]
fn buffer_apply_is_the_sole_raw_mutation_channel() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    let mut offenders = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        for (n, line) in production(&text).lines().enumerate() {
            if is_chained_raw_apply(line) {
                offenders.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(offenders.is_empty(),
        "INV-SEAM: raw Buffer::apply reached through an accessor chain — route it through \
         edit_apply::apply_edit instead:\n{}", offenders.join("\n"));
}

#[test]
fn buffer_apply_is_pub_crate_within_impl_buffer() {
    // C-P1: scope the visibility check to the `impl Buffer { … }` block so it cannot see
    // `Editor::apply` (editor.rs:1072, intentionally still `pub` and returns `EditOutcome`).
    // Slice from `impl Buffer {` to the NEXT top-level `impl `.
    let editor = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/editor.rs")).unwrap();
    let start = editor.find("\nimpl Buffer {").expect("impl Buffer block must exist");
    let rest = &editor[start + 1..];                       // drop the leading '\n'
    let end = rest[1..].find("\nimpl ").map(|i| i + 1).unwrap_or(rest.len());
    let impl_buffer = &rest[..end];                        // the `impl Buffer { … }` body only
    // Sanity: the slice really is impl Buffer and excludes impl Editor's apply.
    assert!(impl_buffer.starts_with("impl Buffer {"), "slice must start at impl Buffer");
    assert!(!impl_buffer.contains("impl Editor"), "slice must not reach impl Editor");
    assert!(impl_buffer.contains("pub(crate) fn apply(&mut self, txn:"),
        "INV-SEAM: Buffer::apply must be pub(crate) (compiler blocks out-of-crate bypass)");
    // A bare `pub fn apply(&mut self, txn:` inside impl Buffer would re-open the external bypass.
    // (`pub(crate) fn apply…` does NOT contain the substring `pub fn apply…`, so this is exact.)
    assert!(!impl_buffer.contains("pub fn apply(&mut self, txn:"),
        "INV-SEAM: Buffer::apply must NOT be bare `pub` — a widen re-opens the external bypass");
}
