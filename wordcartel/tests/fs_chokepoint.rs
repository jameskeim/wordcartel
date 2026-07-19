//! C5 §2.3 — the filesystem-chokepoint guard.
//!
//! Scope is defined by a RULE, not a list: a production call in the `wordcartel` crate that
//! reads file content, enumerates a directory, probes metadata, or mutates durably goes
//! through `fsx::Fs`. This test enforces it by scanning source text and failing on any raw
//! filesystem access not in the allow-list below, where every entry cites the exemption
//! clause it claims.
//!
//! HONEST LIMITS (spec §2.3): the scan is textual, so it can flag a token in a comment or
//! string; `#[cfg(test)]` stripping is heuristic; and the import gate covers the ORDINARY
//! `std::fs` import spellings, not nested-group / renamed-in-group / leading-root `::std::fs`
//! forms. Those gaps are disclosed rather than papered over — closing them needs `use`-tree
//! parsing (a dev-dependency and a mini Rust parser), which was weighed and declined. This is
//! a high-coverage drift alarm, not a completeness proof.

use std::path::{Path, PathBuf};

/// Modules that are WHOLLY exempt, by a clause covering every raw call in the file.
///
/// EXACTLY ONE ENTRY, and that is the honest number.
///
/// `fsx.rs` IS the seam (clause (d)) — every raw call in it is the implementation the rule
/// is defined in terms of, so a per-hit marker on each would be noise.
///
/// Deliberately NOT listed, though earlier drafts listed them:
///   * `filter.rs` and `harper_ls.rs` — verified to contain ZERO raw filesystem calls. Their
///     clause-(a) exemption covers what the CHILD PROCESS does, which this scanner cannot
///     see and does not attempt to. Listing them would imply they hold exempt raw calls.
///   * `recovery.rs` — verified to contain zero raw filesystem calls in production; the panic
///     dump goes through `swap::write_atomic` -> `fsx::atomic_replace`. It was listed as
///     "(d)-adjacent", which conflated two different things: it IS an ownership exception
///     (it cannot take an injected `Arc`, see §5.2's ownership table) but it is NOT a
///     chokepoint exception, because it never bypasses the seam.
///   * `swap.rs`, `settings.rs`, `diagnostics_run.rs`, `session_restore.rs`, `export.rs` —
///     these DO hold exempt raw calls, but only a few each, so they use per-hit markers. A
///     whole-file entry would let a new in-scope call inherit the exemption silently.
const EXEMPT_MODULES: &[(&str, &str)] = &[
    ("src/fsx.rs", "(d) the seam's own implementation"),
];

/// Per-hit exemption marker, placed on the offending line or the line directly above it:
///
/// ```ignore
/// // fs-chokepoint-allow: (b) directory provisioning for the state dir
/// std::fs::create_dir_all(&dir)?;
/// ```
///
/// WHY PER-HIT RATHER THAN PER-FILE. An earlier version of this test allow-listed whole
/// FILES, which meant a NEW in-scope raw call added to `swap.rs` or `export.rs` passed
/// silently — while the task claimed "a new raw call fails the build until routed or
/// allow-listed". Those cannot both be true, and the claim was the false one. A marker has
/// to be written deliberately, sits where the reader is, and names the clause it claims; a
/// new call has no marker and therefore fails.
const ALLOW_MARKER: &str = "fs-chokepoint-allow:";

/// A marker must name a clause — `(a)` through `(g)`. A bare marker is not an exemption,
/// it is an unexplained silence.
fn marker_names_a_clause(line: &str) -> bool {
    // Requires EXACTLY `(x)` where x is one clause letter — not merely "starts with `(` and
    // the next char is in a..=g", which would accept `(gibberish`. The mechanism must
    // validate what its name asserts.
    line.split_once(ALLOW_MARKER)
        .map(|(_, rest)| {
            let r = rest.trim_start().as_bytes();
            r.len() >= 3 && r[0] == b'(' && r[2] == b')' && (b'a'..=b'g').contains(&r[1])
        })
        .unwrap_or(false)
}

/// Inherent `Path` methods that touch the filesystem. A CLOSED, std-defined set: it does not
/// drift as this codebase changes, only if the standard library adds a method. Both call
/// syntaxes are matched — `.method(` and UFCS `Path::method(` — because a dot-call scan
/// misses `Path::metadata(p)` entirely.
const PATH_FS_METHODS: &[&str] = &[
    "metadata", "symlink_metadata", "canonicalize", "read_link", "read_dir",
    "exists", "try_exists", "is_file", "is_dir", "is_symlink",
];

/// Import spellings that bring `std::fs` into scope. Layer 1 — the sound layer for anything
/// reached through an import, because Rust REQUIRES one of these for a short-form `fs::…` or
/// a bare `File::open` call.
fn has_std_fs_import(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with("//") { return false; }
    // `use std::fs;`, `use std::fs::File;`, `use std::fs::OpenOptions;`, `use std::fs as x;`
    // — all share this prefix. Deliberately NOT anchored on a trailing `;`, which would miss
    // every type import.
    if t.starts_with("use std::fs") { return true; }
    // Flat grouped: `use std::{fs, io};` — the literal `use std::fs` never appears.
    if t.starts_with("use std::{") && t.contains("fs") { return true; }
    false
}

/// Drop the trailing `#[cfg(test)] mod tests { … }` block, and NOTHING else.
///
/// CRITICAL: strip on the module-level `#[cfg(test)]` + `mod tests` PAIR, never on a bare
/// `#[cfg(test)]` attribute. `app.rs` carries test-only `use` declarations under
/// `#[cfg(test)]` at lines 10/14/16 — an attribute-only cut discards ~99% of production
/// `app.rs`, INCLUDING the `settings::perform_settings_save` call site that is the one real
/// seam bypass in the tree. A scanner that cannot see `app.rs` cannot enforce anything about
/// the largest hub in the crate, while reporting clean.
///
/// This exact bug occurred twice during authoring — once in the sweep script used to audit
/// the tree, once here — so it is guarded by a planted sample below, not just a comment.
fn strip_test_modules(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut lines = src.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim_start() == "#[cfg(test)]" {
            // Look ahead: only a `mod tests`-shaped item ends production code.
            if lines.peek().is_some_and(|n| n.trim_start().starts_with("mod tests")) {
                break;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn offenders_in(src: &str) -> Vec<String> {
    let prod = strip_test_modules(src);
    let lines: Vec<&str> = prod.lines().collect();
    let mut out = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("//") { continue; }
        let mut hit: Option<String> = None;
        if has_std_fs_import(line) {
            hit = Some("std::fs import".to_string());
        } else if t.contains("std::fs::") {
            hit = Some("fully-qualified std::fs::".to_string());
        } else if t.contains("OpenOptions") {
            hit = Some("OpenOptions".to_string());
        } else {
            for m in PATH_FS_METHODS {
                if t.contains(&format!(".{m}(")) || t.contains(&format!("Path::{m}(")) {
                    hit = Some(format!("inherent Path::{m}"));
                    break;
                }
            }
        }
        let Some(what) = hit else { continue };
        // PER-HIT exemption: a clause-naming marker on this line or the one above.
        let marked_here = marker_names_a_clause(line);
        let marked_above = n > 0 && marker_names_a_clause(lines[n - 1]);
        if marked_here || marked_above { continue; }
        out.push(format!("  line {}: {what} — {}", n + 1, t));
    }
    out
}

fn crate_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rel(p: &Path) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    format!("src/{}", p.strip_prefix(root.join("src")).expect("under src").display())
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).expect("read src").flatten() {
        let p = e.path();
        if p.is_dir() { walk(&p, out); }
        else if p.extension().and_then(|x| x.to_str()) == Some("rs") { out.push(p); }
    }
}

#[test]
fn production_sources_route_filesystem_access_through_the_seam() {
    let mut files = Vec::new();
    walk(&crate_src(), &mut files);
    files.sort();

    let mut failures = Vec::new();
    for f in files {
        let name = rel(&f);
        // e2e.rs is test-only by nature; test_support.rs hosts FaultFs.
        if name == "src/e2e.rs" || name == "src/test_support.rs" { continue; }
        if EXEMPT_MODULES.iter().any(|(a, _)| *a == name) { continue; }
        let src = std::fs::read_to_string(&f).expect("read source");
        let hits = offenders_in(&src);
        if !hits.is_empty() {
            failures.push(format!("{name}:\n{}", hits.join("\n")));
        }
    }

    assert!(failures.is_empty(),
        "raw filesystem access with no exemption marker.\n\n{}\n\n\
         Route each through `fsx::Fs` (spec §5.2), or — if it falls under a §2.3 exemption \
         clause — put a marker on the line or directly above it naming that clause:\n\
         \x20   // fs-chokepoint-allow: (b) directory provisioning for the state dir\n\n\
         Whole-file exemption (EXEMPT_MODULES) is for files where EVERY raw call shares one \
         clause; it is not a way to silence a single new call.",
        failures.join("\n\n"));
}

// ---------------------------------------------------------------------------
// Self-check: one planted evasion per detection route.
//
// A self-check that plants only a fully-qualified call proves layer 2 and NOTHING about
// layers 1 or 3 — the vacuous-guardrail failure. Each sample below is invisible to the
// routes that do not target it, so all four are required.
//
// NOTE: this proves the layers work on the spellings they TARGET. It is not evidence that
// the disclosed gaps (nested-group / renamed-in-group / `::std::fs` imports) are caught.
// ---------------------------------------------------------------------------

#[test]
fn scanner_detects_every_evasion_route() {
    // FAIL-VERIFY: drop the import-gate layer (or the UFCS pattern), watch the corresponding
    // row fail, then revert. A scanner that silently matches nothing passes every other test.
    let cases: &[(&str, &str)] = &[
        ("fully-qualified", "fn f(p: &std::path::Path) { let _ = std::fs::read(p); }"),
        ("aliased import",  "use std::fs;\nfn f(p: &std::path::Path) { let _ = fs::write(p, b\"x\"); }"),
        ("inherent dot",    "fn f(p: &std::path::Path) { let _ = p.symlink_metadata(); }"),
        ("inherent UFCS",   "fn f(p: &std::path::Path) { let _ = std::path::Path::metadata(p); }"),
    ];
    for (label, src) in cases {
        assert!(!offenders_in(src).is_empty(),
            "the scanner missed the {label} evasion — this route is unguarded:\n{src}");
    }
}

#[test]
fn scanner_sees_production_code_below_an_early_cfg_test_attribute() {
    // THE ROUTE THAT ACTUALLY OCCURRED — twice, in two different tools.
    //
    // `app.rs` has test-only `use` declarations under `#[cfg(test)]` near the TOP of the
    // file. A scanner that cuts at the first `#[cfg(test)]` attribute discards essentially
    // all of production `app.rs` — including the one real seam bypass in the tree — and
    // reports clean.
    //
    // BIDIRECTIONAL BY CONSTRUCTION. The two planted calls are detectable by DIFFERENT
    // routes, so each direction of regression breaks a different assertion:
    //   * production `std::fs::read` — only reachable if stripping does NOT cut early;
    //   * in-test `p.symlink_metadata()` — an INHERENT call with no `use std::fs` anywhere
    //     in that block, so its only detection route is the scanner failing to strip at all.
    //
    // FAIL-VERIFY, BOTH DIRECTIONS:
    //   1. Regress `strip_test_modules` to cut at the first `#[cfg(test)]` → the first
    //      assertion fails (production code never scanned).
    //   2. Regress it to strip nothing → the third assertion fails (the in-test call leaks).
    let src = "\
#[cfg(test)]\n\
use std::collections::BTreeMap;\n\
\n\
fn production_code_far_below(p: &std::path::Path) {\n\
    let _ = std::fs::read(p);\n\
}\n\
\n\
#[cfg(test)]\n\
mod tests {\n\
    fn helper(p: &std::path::Path) { let _ = p.symlink_metadata(); }\n\
}\n";
    let hits = offenders_in(src);
    assert!(hits.iter().any(|h| h.contains("std::fs::")),
        "the scanner stopped at the test-only import and never reached production code — \
         this is the defect that hid `app.rs`'s seam bypass:\n{hits:?}");
    assert_eq!(hits.len(), 1,
        "exactly one offender: the production call, nothing from `mod tests`: {hits:?}");
    assert!(!hits.iter().any(|h| h.contains("symlink_metadata")),
        "the `mod tests` body must still be stripped — if this fires, stripping regressed to \
         doing nothing and every test helper now counts as production: {hits:?}");
}

#[test]
fn a_per_hit_marker_exempts_only_its_own_line() {
    // The per-file allow-list this replaced let a NEW raw call inherit an existing file's
    // exemption and pass silently. A marker exempts one call and nothing else.
    //
    // FAIL-VERIFY: make the marker check file-wide (or drop it), watch the second
    // assertion fail.
    let src = "\
fn provision(d: &std::path::Path) {\n\
    // fs-chokepoint-allow: (b) directory provisioning for the state dir\n\
    let _ = std::fs::create_dir_all(d);\n\
    let _ = std::fs::read(d);\n\
}\n";
    let hits = offenders_in(src);
    assert!(!hits.iter().any(|h| h.contains("create_dir_all")),
        "the marked line is exempt: {hits:?}");
    assert!(hits.iter().any(|h| h.contains("std::fs::read")),
        "the UNMARKED call on the next line must still fail — an exemption is per-hit, not \
         per-file, and this is exactly what the old whole-file list got wrong: {hits:?}");
}

#[test]
fn a_marker_without_a_clause_is_not_an_exemption() {
    // A bare marker is an unexplained silence. Every exemption names the clause it claims.
    for bad in ["trust me", "(gibberish", "(z) not a clause", "()", "(a"] {
        let src = format!(
            "fn sneaky(p: &std::path::Path) {{\n    // fs-chokepoint-allow: {bad}\n    \
             let _ = std::fs::read(p);\n}}\n");
        assert!(!offenders_in(&src).is_empty(),
            "marker {bad:?} names no valid (a)-(g) clause and must NOT silence the hit");
    }
    // …and a well-formed one does.
    let ok = "fn f(p: &std::path::Path) {\n    // fs-chokepoint-allow: (c) canonicalize\n    \
              let _ = std::fs::read(p);\n}\n";
    assert!(offenders_in(ok).is_empty(), "a clause-naming marker exempts its line");
}

#[test]
fn scanner_ignores_ordinary_code_and_test_modules() {
    // A false positive costs one allow-list line, so over-matching is survivable — but the
    // scanner must not flag code with no filesystem access at all, or the list becomes noise.
    assert!(offenders_in("fn f(x: usize) -> usize { x + 1 }").is_empty());
    // Everything from the module-level #[cfg(test)] marker onward is stripped.
    let with_tests = "fn f() {}\n#[cfg(test)]\nmod tests {\n  use std::fs;\n}\n";
    assert!(offenders_in(with_tests).is_empty(), "test modules are out of scope by the rule");
}
