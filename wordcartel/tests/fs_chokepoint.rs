//! C5 §2.3 — the filesystem-chokepoint guard.
//!
//! Scope is defined by a RULE, not a list: a production call in the `wordcartel` crate that
//! reads file content, enumerates a directory, probes metadata, or mutates durably goes
//! through `fsx::Fs`. This test enforces it by scanning source text and failing on any raw
//! filesystem access not in the allow-list below, where every entry cites the exemption
//! clause it claims.
//!
//! FOUR LAYERS, not three. Layers 1-3 catch RAW filesystem access (`use std::fs`, a
//! fully-qualified `std::fs::…`, an inherent `Path` method). Layer 4 catches the
//! **wrapper-bypass class**: production code calling an IN-CRATE wrapper that constructs
//! `RealFs` internally. That call contains no raw token at all, so layers 1-3 are blind to
//! it — which is how the file picker shipped holding `ctx.fs` and discarding it, with this
//! gate reporting clean through 26 tasks and two review layers. Layer 4's wrapper set is
//! derived from the source, never listed, and its markers use the `(w)` tag.
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
///   * `harper_ls.rs` — verified to contain ZERO raw filesystem calls. Its clause-(a)
///     exemption covers what the CHILD PROCESS does, which this scanner cannot see and does
///     not attempt to. Listing it would imply it holds an exempt raw call.
///   * `filter.rs` — NOT zero: it carries one per-hit clause-(a) marker, on
///     `spawn_stdin_writer`'s `std::fs::File` stdin-pipe-handle parameter. That is a TYPE in a
///     signature, not a filesystem call, which is why a whole-file entry still isn't warranted
///     — a NEW raw call added to this file must still carry its own marker or fail the gate.
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

// ---------------------------------------------------------------------------
// Layer 4 — the WRAPPER-BYPASS class.
//
// WHY THIS LAYER EXISTS, stated plainly: the three layers above scan for raw `std::fs`
// tokens, and a production call into an IN-CRATE wrapper that constructs `RealFs` inside
// itself contains no such token. So `crate::file::open(p)` — which reaches `RealFs`
// unconditionally — passed the gate while bypassing the injected seam entirely.
//
// That is not hypothetical. It is exactly how the picker's Open path shipped holding
// `ctx.fs` and discarding it, through 26 tasks and two review layers, with this gate
// reporting clean. A gate that does not enforce what it claims is worse than no gate,
// because it is *believed*.
//
// The wrapper set is DERIVED, never listed: any production fn whose own body names
// `fsx::RealFs` outside an `Arc::new(...)` composition root is a wrapper. Deriving it means
// a wrapper added next year is covered the day it is written — a hand-written list would
// have to be remembered, and the whole failure being fixed here is a thing nobody remembered.
// `Arc::new(RealFs)` is excluded on purpose: that is the shape of a composition root, which
// HANDS the seam downstream rather than swallowing it.
// ---------------------------------------------------------------------------

/// A `RealFs`-hardcoding wrapper: the module it lives in and its name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wrapper { module: String, name: String }

/// True for a production line that pins `RealFs` inline (`&crate::fsx::RealFs`,
/// `crate::fsx::RealFs.stat(..)`) rather than composing it into an `Arc` for injection.
fn pins_realfs(line: &str) -> bool {
    let t = line.trim();
    if t.starts_with("//") { return false; }
    t.contains("fsx::RealFs") && !t.contains("Arc::new(")
}

/// Collect the `RealFs`-hardcoding wrappers defined in one production module.
///
/// Item-level scan: a top-level `fn` (column 0, optionally `pub`/`pub(crate)`) owns every
/// line until the next top-level `fn`. Deliberately coarse — it over-attributes a nested
/// helper's `RealFs` to its enclosing fn, which can only make the gate stricter, never laxer.
fn wrappers_in(module: &str, src: &str) -> Vec<Wrapper> {
    let prod = strip_test_modules(src);
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    let mut pinned = false;
    let flush = |cur: &mut Option<String>, pinned: &mut bool, out: &mut Vec<Wrapper>| {
        if let (Some(name), true) = (cur.take(), *pinned) {
            out.push(Wrapper { module: module.to_string(), name });
        }
        *pinned = false;
    };
    for line in prod.lines() {
        if let Some(name) = top_level_fn_name(line) {
            flush(&mut current, &mut pinned, &mut out);
            current = Some(name);
        }
        if pins_realfs(line) { pinned = true; }
    }
    flush(&mut current, &mut pinned, &mut out);
    out
}

/// The name of a top-level (column-0) `fn` declared on this line, if any.
fn top_level_fn_name(line: &str) -> Option<String> {
    if line.starts_with(' ') || line.starts_with('\t') { return None; }
    let rest = line
        .strip_prefix("pub ")
        .or_else(|| line.strip_prefix("pub(crate) ").or_else(|| line.strip_prefix("pub(super) ")))
        .unwrap_or(line);
    let rest = rest.strip_prefix("fn ")?;
    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    if name.is_empty() { None } else { Some(name) }
}

/// Production hits in `module`'s source that bypass the seam via a `RealFs` wrapper —
/// either by CALLING one (`crate::file::open(..)` / `file::open(..)`) or by pinning
/// `RealFs` inline itself. Both need a marker; each marker names why that site is legitimate.
///
/// A wrapper's OWN body is not a bypass of itself, so calls are matched only in qualified
/// (`module::name(`) form. Unqualified same-module calls are a disclosed gap — the same
/// class of gap the header already discloses for `use`-tree spellings.
fn wrapper_offenders_in(module: &str, src: &str, wrappers: &[Wrapper]) -> Vec<String> {
    let prod = strip_test_modules(src);
    let lines: Vec<&str> = prod.lines().collect();
    let mut out = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("//") { continue; }
        let mut hit: Option<String> = None;
        if pins_realfs(line) {
            hit = Some("pins `RealFs` inline — the injected seam cannot reach here".to_string());
        } else {
            for w in wrappers {
                if w.module == module { continue; } // a wrapper's own module is its definition site
                if t.contains(&format!("{}::{}(", w.module, w.name)) {
                    hit = Some(format!("calls `{}::{}` — a wrapper that hardcodes `RealFs`",
                        w.module, w.name));
                    break;
                }
            }
        }
        let Some(what) = hit else { continue };
        let marked_here = marker_allows_a_wrapper(lines[n]);
        let marked_above = n > 0 && marker_allows_a_wrapper(lines[n - 1]);
        if marked_here || marked_above { continue; }
        out.push(format!("  line {}: {what} — {}", n + 1, t));
    }
    out
}

/// A layer-4 marker names either a §2.3 clause `(a)`–`(g)`, or the tag **`(w)`** —
/// *wrapper by decision*: a `RealFs`-hardcoding wrapper whose call site is deliberately not
/// injected. `(w)` is NOT a new §2.3 clause and does not widen the rule; it records the two
/// standing project decisions that put a site outside injection — config-class reads (small
/// config/theme/session files, not document-class content — CLAUDE.md) and ownership
/// exceptions (no `fs` in scope, spec §5.2's ownership table).
///
/// `(w)` alone is not enough: it must be followed by prose. A tag with no reason is the
/// unexplained silence this whole marker convention exists to prevent, and `(w)` is the tag
/// most likely to be reached for reflexively.
fn marker_allows_a_wrapper(line: &str) -> bool {
    if marker_names_a_clause(line) { return true; }
    line.split_once(ALLOW_MARKER)
        .map(|(_, rest)| {
            let r = rest.trim_start();
            r.starts_with("(w)") && !r["(w)".len()..].trim().is_empty()
        })
        .unwrap_or(false)
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

/// Collect every production source in scope for the scan, as `(module-name, source)`.
fn production_modules() -> Vec<(String, String)> {
    let mut files = Vec::new();
    walk(&crate_src(), &mut files);
    files.sort();
    files.into_iter()
        .filter(|f| {
            let name = rel(f);
            name != "src/e2e.rs" && name != "src/test_support.rs"
                && !EXEMPT_MODULES.iter().any(|(a, _)| *a == name)
        })
        .map(|f| {
            let module = f.file_stem().and_then(|s| s.to_str()).expect("stem").to_string();
            (module, std::fs::read_to_string(&f).expect("read source"))
        })
        .collect()
}

#[test]
fn production_call_sites_do_not_bypass_the_seam_through_a_realfs_wrapper() {
    // THE GAP THAT LET C5's HEADLINE BUG SHIP. See the layer-4 header above.
    let modules = production_modules();
    let wrappers: Vec<Wrapper> = modules.iter()
        .flat_map(|(m, src)| wrappers_in(m, src))
        .collect();

    // The derivation must actually find wrappers. If a refactor renames `fn` shapes or
    // `RealFs` moves, `wrappers` silently empties and every call site below passes for the
    // wrong reason — the vacuous-guardrail failure this effort kept producing.
    assert!(wrappers.iter().any(|w| w.module == "file" && w.name == "open"),
        "the wrapper derivation found no `file::open` — it has stopped seeing wrappers and \
         this whole layer is now vacuous: {wrappers:?}");

    let mut failures = Vec::new();
    for (module, src) in &modules {
        let hits = wrapper_offenders_in(module, src, &wrappers);
        if !hits.is_empty() {
            failures.push(format!("src/{module}.rs:\n{}", hits.join("\n")));
        }
    }

    assert!(failures.is_empty(),
        "production code reaches the filesystem through a `RealFs`-hardcoding wrapper, \
         bypassing the injected seam.\n\n{}\n\n\
         If an injected `fs` is in scope, THREAD IT — call the `*_with_fs` seam instead \
         (this is C1: the picker held `ctx.fs` and dropped it).\n\
         If the site is deliberately not injected, mark it on the line or directly above:\n\
         \x20   // fs-chokepoint-allow: (w) config-class read — not document content\n\n\
         `(w)` must be followed by a reason. A §2.3 clause letter (a)-(g) also works where \
         one genuinely applies.",
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
fn layer_four_detects_a_wrapper_call_that_layers_one_to_three_are_blind_to() {
    // The C1 SHAPE, reduced. `file.rs` defines `open` as a `RealFs` wrapper; another module
    // calls it. There is no `std::fs` token anywhere in the caller — run the raw-access
    // scanner over it and it reports clean, which is precisely the false all-clear this
    // layer exists to end.
    let file_rs = "\
pub fn open(p: &std::path::Path) -> Vec<u8> {\n\
    open_with_fs(&crate::fsx::RealFs, p)\n\
}\n";
    let caller = "\
pub fn enter(fs: &dyn crate::fsx::Fs, p: &std::path::Path) {\n\
    let _ = crate::file::open(p);\n\
}\n";
    let wrappers = wrappers_in("file", file_rs);
    assert_eq!(wrappers, vec![Wrapper { module: "file".into(), name: "open".into() }],
        "the wrapper must be DERIVED from `file.rs`'s body, not assumed");

    assert!(offenders_in(caller).is_empty(),
        "premise: layers 1-3 see nothing here — no raw `std::fs`, no inherent Path method. \
         If this ever fails, the layer-4 rationale needs rewriting, not the code.");
    let hits = wrapper_offenders_in("file_browser", caller, &wrappers);
    assert!(hits.iter().any(|h| h.contains("file::open")),
        "layer 4 must flag the wrapper call that layers 1-3 cannot see: {hits:?}");
}

#[test]
fn layer_four_wrapper_derivation_excludes_an_arc_composition_root() {
    // `Arc::new(RealFs)` is the composition root — it HANDS the seam downstream. Treating it
    // as a wrapper would flag `app::run` and, transitively, `main`, which is noise that
    // teaches readers to stop reading the failure.
    let root = "\
pub fn run(cli: Cli) {\n\
    let fs: std::sync::Arc<dyn crate::fsx::Fs + Send + Sync> =\n\
        std::sync::Arc::new(crate::fsx::RealFs);\n\
    drive(&*fs);\n\
}\n";
    assert!(wrappers_in("app", root).is_empty(),
        "an Arc composition root is not a wrapper: {:?}", wrappers_in("app", root));
}

#[test]
fn layer_four_markers_must_carry_a_reason() {
    // `(w)` is the tag a reader reaches for reflexively, so a bare one must not silence a
    // hit. FAIL-VERIFY: drop the emptiness check in `marker_allows_a_wrapper` and the loop
    // below fails on `(w)`.
    let wrappers = vec![Wrapper { module: "file".into(), name: "open".into() }];
    for bad in ["(w)", "(w)   ", "trust me", "(z) nope"] {
        let src = format!("fn f(p: &std::path::Path) {{\n    // fs-chokepoint-allow: {bad}\n    \
                           let _ = crate::file::open(p);\n}}\n");
        assert!(!wrapper_offenders_in("caller", &src, &wrappers).is_empty(),
            "marker {bad:?} carries no reason and must NOT silence a wrapper bypass");
    }
    let ok = "fn f(p: &std::path::Path) {\n    \
              // fs-chokepoint-allow: (w) config-class read — not document content\n    \
              let _ = crate::file::open(p);\n}\n";
    assert!(wrapper_offenders_in("caller", ok, &wrappers).is_empty(),
        "a `(w)` marker with a reason exempts its line");
    // A genuine §2.3 clause letter works too, for a site a clause really covers.
    let clause = "fn f(p: &std::path::Path) {\n    \
                  // fs-chokepoint-allow: (a) subprocess-owned IO\n    \
                  let _ = crate::file::open(p);\n}\n";
    assert!(wrapper_offenders_in("caller", clause, &wrappers).is_empty(),
        "a clause letter is also a valid layer-4 marker");
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
