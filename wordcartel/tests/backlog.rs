//! Backlog tracking gate — the drift-proof check over `backlog.toml`.
//!
//! Source of truth for item STATE is the repo-root `backlog.toml`; this test renders it
//! into `BACKLOG.md` (same repo root) with one `render()` fn used for BOTH generation and
//! verification, and enforces invariants I1–I7 (design spec §7). Regenerate the dashboard
//! with `BLESS_BACKLOG=1 cargo test -p wordcartel --test backlog`.

use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const STATUSES: &[&str] = &["triage", "needs-design", "ready", "in-progress", "watch", "shipped", "dropped"];
const KINDS: &[&str] = &["feature", "bug", "debt", "chore", "research"];
const SIZES: &[&str] = &["S", "SM", "M", "L", "XL", "TBD"];
const THEMES: &[&str] = &["A", "B", "C", "D", "E", "H", "M", "R", "S", "P"];

#[derive(Deserialize, Default)]
struct Manifest {
    #[serde(default)]
    item: Vec<Item>,
}

#[derive(Deserialize, Clone)]
struct Item {
    id: String,
    title: String,
    status: String,
    kind: String,
    size: String,
    theme: String,
    hook: String,
    doc: String,
    #[serde(default)]
    blocks_effort_p: bool,
    #[serde(default)]
    depends_on: Vec<String>,
    created: String,
    #[serde(default)]
    shipped_commit: Option<String>,
    #[serde(default)]
    shipped_date: Option<String>,
    #[serde(default)]
    dropped_reason: Option<String>,
}

fn parse(s: &str) -> Manifest {
    toml::from_str(s).unwrap_or_else(|e| panic!("backlog manifest parse error: {e}"))
}

/// True for a `YYYY-MM-DD` date shape (digits + dashes at the right offsets).
fn is_ymd(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// Returns a list of invariant violations (I1–I3, I6). Empty ⇒ valid.
fn validate(m: &Manifest) -> Vec<String> {
    let mut errs = Vec::new();
    let mut seen: HashSet<&str> = HashSet::new();
    for it in &m.item {
        if !seen.insert(it.id.as_str()) {
            errs.push(format!("I1 duplicate id: {}", it.id));
        }
        if !STATUSES.contains(&it.status.as_str()) {
            errs.push(format!("I2 bad status '{}' on {}", it.status, it.id));
        }
        if !KINDS.contains(&it.kind.as_str()) {
            errs.push(format!("I2 bad kind '{}' on {}", it.kind, it.id));
        }
        if !SIZES.contains(&it.size.as_str()) {
            errs.push(format!("I2 bad size '{}' on {}", it.size, it.id));
        }
        if !THEMES.contains(&it.theme.as_str()) {
            errs.push(format!("I2 bad theme '{}' on {}", it.theme, it.id));
        }
        if !is_ymd(&it.created) {
            errs.push(format!("I2 {} has non-YYYY-MM-DD created '{}'", it.id, it.created));
        }
        match it.status.as_str() {
            "shipped" if it.shipped_commit.is_none() || it.shipped_date.is_none() => {
                errs.push(format!("I3 {} is shipped but missing shipped_commit/shipped_date", it.id));
            }
            "dropped" if it.dropped_reason.is_none() => {
                errs.push(format!("I3 {} is dropped but missing dropped_reason", it.id));
            }
            _ => {}
        }
        if let Some(d) = &it.shipped_date {
            if !is_ymd(d) {
                errs.push(format!("I2 {} has non-YYYY-MM-DD shipped_date '{}'", it.id, d));
            }
        }
    }
    let ids: HashSet<&str> = m.item.iter().map(|i| i.id.as_str()).collect();
    for it in &m.item {
        for d in &it.depends_on {
            if !ids.contains(d.as_str()) {
                errs.push(format!("I6 {} depends_on unknown id '{}'", it.id, d));
            }
        }
    }
    errs
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("wordcartel/ has a parent (repo root)")
        .to_path_buf()
}

fn size_rank(s: &str) -> usize {
    SIZES.iter().position(|x| *x == s).unwrap_or(SIZES.len())
}

fn is_open(status: &str) -> bool {
    !matches!(status, "shipped" | "dropped")
}

/// Renders the canonical `BACKLOG.md`. Pure fn — same output for same input, used for BOTH
/// generation (BLESS) and verification (I7).
fn render(m: &Manifest) -> String {
    let mut out = String::new();
    out.push_str("<!-- GENERATED from backlog.toml — do not edit by hand. -->\n");
    out.push_str("<!-- Regenerate: BLESS_BACKLOG=1 cargo test -p wordcartel --test backlog -->\n\n");
    out.push_str("# Backlog\n\n");

    let open: Vec<&Item> = m.item.iter().filter(|i| is_open(&i.status)).collect();
    let shipped: Vec<&Item> = m.item.iter().filter(|i| i.status == "shipped").collect();
    let dropped: Vec<&Item> = m.item.iter().filter(|i| i.status == "dropped").collect();

    out.push_str(&format!(
        "**{} open · {} shipped · {} dropped**\n\n",
        open.len(),
        shipped.len(),
        dropped.len()
    ));
    let blocking = open.iter().filter(|i| i.blocks_effort_p).count();
    out.push_str(&format!("Blocking Effort P: **{blocking}**\n\n"));

    let mut open_sorted = open.clone();
    open_sorted.sort_by(|a, b| {
        b.blocks_effort_p
            .cmp(&a.blocks_effort_p) // true (blocking) first
            .then_with(|| size_rank(&a.size).cmp(&size_rank(&b.size)))
            .then_with(|| a.theme.cmp(&b.theme))
            .then_with(|| a.id.cmp(&b.id))
    });
    out.push_str("## Open\n\n");
    out.push_str("| id | title | status | kind | size | P? | hook |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    for it in &open_sorted {
        let p = if it.blocks_effort_p { "🚩" } else { "" };
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} |\n",
            it.id, it.title, it.status, it.kind, it.size, p, it.hook
        ));
    }
    out.push('\n');

    let mut ship_sorted = shipped.clone();
    ship_sorted.sort_by(|a, b| b.shipped_date.cmp(&a.shipped_date).then_with(|| a.id.cmp(&b.id)));
    out.push_str(&format!("## Shipped\n\n<details><summary>{} shipped</summary>\n\n", ship_sorted.len()));
    out.push_str("| id | title | date | commit |\n|---|---|---|---|\n");
    for it in &ship_sorted {
        out.push_str(&format!(
            "| {} | {} | {} | {} |\n",
            it.id,
            it.title,
            it.shipped_date.as_deref().unwrap_or(""),
            it.shipped_commit.as_deref().unwrap_or(""),
        ));
    }
    out.push_str("\n</details>\n");

    if !dropped.is_empty() {
        let mut drop_sorted = dropped.clone();
        drop_sorted.sort_by(|a, b| a.id.cmp(&b.id));
        out.push_str(&format!("\n## Dropped\n\n<details><summary>{} dropped</summary>\n\n", drop_sorted.len()));
        out.push_str("| id | title | reason |\n|---|---|---|\n");
        for it in &drop_sorted {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                it.id,
                it.title,
                it.dropped_reason.as_deref().unwrap_or("")
            ));
        }
        out.push_str("\n</details>\n");
    }
    out
}

const GOOD_FIXTURE: &str = r#"
[[item]]
id = "A9"
title = "Wrap Column state-in-label"
status = "needs-design"
kind = "feature"
size = "S"
theme = "A"
hook = "Rename Set Wrap Column to stateful Wrap Column: value."
doc = "ux-backlog.md#a9"
created = "2026-07-09"

[[item]]
id = "E6"
title = "Splash / start screen"
status = "shipped"
kind = "feature"
size = "M"
theme = "E"
hook = "Startup splash; view.splash + --no-splash."
doc = "ux-backlog.md#e6"
created = "2026-07-09"
shipped_commit = "242c987"
shipped_date = "2026-07-09"
"#;

#[test]
fn valid_manifest_has_no_violations() {
    let m = parse(GOOD_FIXTURE);
    assert_eq!(validate(&m), Vec::<String>::new());
}

#[test]
fn validate_catches_dup_id_bad_enum_and_missing_ship_fields() {
    let bad = r#"
[[item]]
id = "X1"
title = "dup + bad status"
status = "frobnicate"
kind = "feature"
size = "S"
theme = "A"
hook = "h"
doc = "ux-backlog.md#x1"
created = "2026-07-10"

[[item]]
id = "X1"
title = "shipped without commit"
status = "shipped"
kind = "feature"
size = "S"
theme = "A"
hook = "h"
doc = "ux-backlog.md#x1b"
created = "2026-07-10"

[[item]]
id = "X2"
title = "dangling dep"
status = "ready"
kind = "feature"
size = "S"
theme = "A"
hook = "h"
doc = "ux-backlog.md#x2"
depends_on = ["NOPE"]
created = "2026-07-10"
"#;
    let errs = validate(&parse(bad));
    assert!(errs.iter().any(|e| e.starts_with("I1")), "want dup-id: {errs:?}");
    assert!(errs.iter().any(|e| e.contains("bad status")), "want I2 status: {errs:?}");
    assert!(errs.iter().any(|e| e.starts_with("I3")), "want I3 ship-fields: {errs:?}");
    assert!(errs.iter().any(|e| e.starts_with("I6")), "want I6 dangling dep: {errs:?}");
}

#[test]
fn render_is_deterministic_and_has_expected_sections() {
    let m = parse(GOOD_FIXTURE);
    let a = render(&m);
    let b = render(&m);
    assert_eq!(a, b, "render must be deterministic");
    assert!(a.contains("**1 open · 1 shipped · 0 dropped**"), "counts header:\n{a}");
    assert!(a.contains("| A9 | Wrap Column state-in-label | needs-design |"), "open row:\n{a}");
    assert!(a.contains("| E6 | Splash / start screen | 2026-07-09 | 242c987 |"), "shipped row:\n{a}");
    assert!(a.contains("GENERATED from backlog.toml"), "generated banner:\n{a}");
}

fn load() -> Manifest {
    let p = repo_root().join("backlog.toml");
    let s = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()));
    parse(&s)
}

const DOCS: &[&str] = &["ux-backlog.md", "engineering-health.md", "backlog-archive.md"];

fn read_doc(file: &str) -> String {
    let p = repo_root().join("docs").join(file);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

fn doc_file_of(it: &Item) -> &str {
    it.doc.split('#').next().unwrap_or(&it.doc)
}

/// `<!-- item: ID -->` markers with their 0-based line index.
fn markers_in(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let t = l.trim();
            t.strip_prefix("<!-- item:")
                .and_then(|r| r.strip_suffix("-->"))
                .map(|id| (i, id.trim().to_string()))
        })
        .collect()
}

/// I4 — per file, the `<!-- item: ID -->` markers are EXACTLY the manifest items whose `doc`
/// names that file (every item, any status). Open items live in the live docs; shipped/dropped
/// live in `backlog-archive.md`. Catches orphaned prose (marker, no row), orphaned rows (item,
/// no marker), and an item filed to the wrong doc — i.e. a missed/extra/misplaced transcription.
#[test]
fn i4_items_and_markers_are_bijective_per_file() {
    let m = load();
    for file in DOCS {
        let text = read_doc(file);
        let marker_ids: Vec<String> = markers_in(&text).into_iter().map(|(_, id)| id).collect();
        let marker_set: HashSet<String> = marker_ids.iter().cloned().collect();
        assert_eq!(marker_set.len(), marker_ids.len(), "{file}: duplicate <!-- item --> marker");
        let doc_ids: HashSet<String> = m
            .item
            .iter()
            .filter(|it| doc_file_of(it) == *file)
            .map(|it| it.id.clone())
            .collect();
        let orphan_prose: Vec<&String> = marker_set.difference(&doc_ids).collect();
        let orphan_rows: Vec<&String> = doc_ids.difference(&marker_set).collect();
        assert!(orphan_prose.is_empty(), "{file}: markers with no manifest row: {orphan_prose:?}");
        assert!(orphan_rows.is_empty(), "{file}: manifest rows with no marker: {orphan_rows:?}");
    }
}

/// I5 — the heading each open marker annotates carries no status token. Status is
/// manifest-only; a status word in a heading is a double-source and a drift vector.
#[test]
fn i5_open_item_headings_carry_no_status_token() {
    const BANNED: &[&str] = &[
        "SHIPPED",
        "`needs-design`",
        "`settled-design`",
        "`settled-principle`",
        "`potential-bug`",
        "`watch`",
        "`triage`",
        "`ready`",
        "`in-progress`",
        "`available-today`",
        "`fact-checked`",
    ];
    for file in DOCS {
        let text = read_doc(file);
        let lines: Vec<&str> = text.lines().collect();
        for (idx, _) in markers_in(&text) {
            // The heading is the nearest preceding line that starts with '#'.
            let heading = (0..idx).rev().map(|i| lines[i]).find(|l| l.trim_start().starts_with('#'));
            let heading = heading.unwrap_or_else(|| panic!("{file}:{}: marker with no preceding heading", idx + 1));
            for b in BANNED {
                assert!(
                    !heading.contains(b),
                    "docs/{file}: status token {b} in an open item's heading — status belongs ONLY in backlog.toml:\n  {heading}"
                );
            }
        }
    }
}

#[test]
fn backlog_toml_is_schema_valid() {
    let errs = validate(&load());
    assert!(errs.is_empty(), "backlog.toml invariant violations:\n{}", errs.join("\n"));
}

#[test]
fn backlog_index_is_up_to_date() {
    let expected = render(&load());
    let path = repo_root().join("BACKLOG.md");
    if std::env::var_os("BLESS_BACKLOG").is_some() {
        std::fs::write(&path, &expected).expect("write BACKLOG.md");
        return;
    }
    let actual = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        actual, expected,
        "BACKLOG.md is stale — regenerate with: BLESS_BACKLOG=1 cargo test -p wordcartel --test backlog"
    );
}
