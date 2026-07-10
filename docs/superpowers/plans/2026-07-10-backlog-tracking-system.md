# Backlog Tracking System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace hand-maintained, drift-prone status prose across two backlog docs with a single structured source of truth (`backlog.toml`) + a generated dashboard (`BACKLOG.md`) + a `cargo test` gate that makes staleness a merge failure.

**Architecture:** `backlog.toml` (repo root) is the sole source of truth for each item's *state*. A pure `render()` fn in `wordcartel/tests/backlog.rs` produces `BACKLOG.md` (repo root) and is used for BOTH generation (`BLESS_BACKLOG=1`) and verification, so they cannot diverge. The rich triage prose stays in `docs/ux-backlog.md` + `docs/engineering-health.md`, keyed to the manifest by `<!-- item: ID -->` markers, with status tokens removed. A `#[test]` enforces invariants I1–I7 (see spec §7).

**Tech Stack:** Rust integration test (`serde` derive + `toml 0.8`, both already deps), a POSIX shell wrapper. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-10-backlog-tracking-system-design.md` (authoritative — schema §4, invariants §7, dashboard format §8, rename/merge table §9).

## Global Constraints

- **This is repo tooling, not editor code** — the heavy Codex/Fable review layers do NOT apply; the `cargo test` + workspace-clippy gates are the check.
- **`cargo test` green + `cargo clippy --workspace --all-targets` clean are merge GATEs** (CLAUDE.md). The new test file must be clippy-clean.
- **Do NOT run `cargo fmt`** — hand-match the neighboring style in `wordcartel/tests/module_budgets.rs`.
- **Both `backlog.toml` and `BACKLOG.md` live at the REPO ROOT** (one level ABOVE `wordcartel/`). In the test, repo root = `Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()`.
- **Status lives ONLY in `backlog.toml`** — never in prose headers (invariant I5).
- **Commit/push only when the user explicitly asks** (CLAUDE.md). Commit steps are written below, but hold actual commits until authorized. Every commit ends with the two project trailers verbatim:
  ```
  Co-Authored-By: Claude Opus 4.8 <noreply@anthropic.com>
  Claude-Session: <current session URL>
  ```
- **`em-dash` (—) in prose, never `--`.** No emoji in code.
- **Enum vocabularies (verbatim, spec §4):**
  - `status`: `triage` `needs-design` `ready` `in-progress` `watch` `shipped` `dropped`
  - `kind`: `feature` `bug` `debt` `chore` `research`
  - `size`: `S` `SM` `M` `L` `XL` `TBD`
  - `theme`: `A` `B` `C` `D` `E` `H` `M` `R` `S` `P`

---

## File Structure

| File | Responsibility |
|---|---|
| `backlog.toml` (root, **create**) | Source of truth for item state — one `[[item]]` per entry |
| `BACKLOG.md` (root, **create**, generated) | Human/LLM dashboard: counts + open table + shipped/dropped logs |
| `wordcartel/tests/backlog.rs` (**create**) | The gate: schema types, `render()`, `validate()`, invariant `#[test]`s, `BLESS_BACKLOG` regeneration |
| `scripts/backlog` (**create**, `chmod +x`) | Wrapper: `bless` / `add` / `open` / `shipped` |
| `docs/ux-backlog.md` (**modify**) | Add `<!-- item: ID -->` markers; strip status tokens; delete "Sizing summary" + "Working order"; move Theme-H entries out |
| `docs/engineering-health.md` (**modify**) | Add markers; strip status tokens; receive relocated `H15`/`H16`; split `H1`→`H1`+`H14` |
| `CLAUDE.md` (**modify**) | One-line pointer to `BACKLOG.md` + the `bl:` flow |
| `~/.claude/.../memory/backlog-shorthand-bl.md` (**modify**) | Update `bl:` to the manifest flow |

---

## Task 1: Gate scaffold — schema types, parse, validate (I1–I3, I6)

**Files:**
- Create: `wordcartel/tests/backlog.rs`

**Interfaces:**
- Produces: `struct Manifest { item: Vec<Item> }`, `struct Item { id, title, status, kind, size, theme, hook, doc, blocks_effort_p, depends_on, created, shipped_commit, shipped_date, dropped_reason }`, `fn validate(m: &Manifest) -> Vec<String>` (empty ⇒ valid), `fn parse(s: &str) -> Manifest`.

- [ ] **Step 1: Write the failing test** (fixture-driven — no real data file needed yet)

```rust
//! Backlog tracking gate — the drift-proof check over `backlog.toml`.
//!
//! Source of truth for item STATE is the repo-root `backlog.toml`; this test renders it
//! into `BACKLOG.md` (same repo root) with one `render()` fn used for BOTH generation and
//! verification, and enforces invariants I1–I7 (design spec §7). Regenerate the dashboard
//! with `BLESS_BACKLOG=1 cargo test --test backlog`.

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
        match it.status.as_str() {
            "shipped" if it.shipped_commit.is_none() || it.shipped_date.is_none() => {
                errs.push(format!("I3 {} is shipped but missing shipped_commit/shipped_date", it.id));
            }
            "dropped" if it.dropped_reason.is_none() => {
                errs.push(format!("I3 {} is dropped but missing dropped_reason", it.id));
            }
            _ => {}
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

#[allow(dead_code)] // used by later tasks (load real file)
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("wordcartel/ has a parent (repo root)")
        .to_path_buf()
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
```

- [ ] **Step 2: Run tests to verify they fail (compile-first) / pass**

Run: `cargo test -p wordcartel --test backlog -- --nocapture`
Expected: compiles and PASSES (the fixtures exercise the logic just written). If `render()`-related later tests don't exist yet, that's fine — only these two run.

- [ ] **Step 3: Confirm clippy-clean**

Run: `cargo clippy -p wordcartel --tests`
Expected: no warnings on `backlog.rs`.

- [ ] **Step 4: Commit** (hold until authorized — see Global Constraints)

```bash
git add wordcartel/tests/backlog.rs
git commit  # message: "feat(backlog): manifest schema + validate() gate (I1–I3,I6)" + trailers
```

---

## Task 2: `render()` + dashboard-format pin

**Files:**
- Modify: `wordcartel/tests/backlog.rs`

**Interfaces:**
- Consumes: `Manifest`, `Item` (Task 1).
- Produces: `fn render(m: &Manifest) -> String` — the deterministic `BACKLOG.md` body (spec §8): a counts header, an OPEN table sorted by `(blocks_effort_p desc, size rank, theme, id)`, a collapsed SHIPPED log (newest `shipped_date` first), a collapsed DROPPED list.

- [ ] **Step 1: Write the failing test** (append to `backlog.rs`)

```rust
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

    // Counts header.
    out.push_str(&format!(
        "**{} open · {} shipped · {} dropped**\n\n",
        open.len(),
        shipped.len(),
        dropped.len()
    ));
    let blocking = open.iter().filter(|i| i.blocks_effort_p).count();
    out.push_str(&format!("Blocking Effort P: **{blocking}**\n\n"));

    // Open table.
    let mut open_sorted = open.clone();
    open_sorted.sort_by(|a, b| {
        (!b.blocks_effort_p, b.blocks_effort_p) // placeholder to keep b referenced; real key below
            .cmp(&(false, false))
            .then_with(|| a.blocks_effort_p.cmp(&b.blocks_effort_p).reverse())
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

    // Shipped log (newest first).
    let mut ship_sorted = shipped.clone();
    ship_sorted.sort_by(|a, b| {
        b.shipped_date.cmp(&a.shipped_date).then_with(|| a.id.cmp(&b.id))
    });
    out.push_str("## Shipped\n\n<details><summary>");
    out.push_str(&format!("{} shipped", ship_sorted.len()));
    out.push_str("</summary>\n\n| id | title | date | commit |\n|---|---|---|---|\n");
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
        out.push_str("\n## Dropped\n\n<details><summary>");
        out.push_str(&format!("{} dropped", drop_sorted.len()));
        out.push_str("</summary>\n\n| id | title | reason |\n|---|---|---|\n");
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
```

> **Note on the sort key:** replace the placeholder comparator with the real one below in Step 3 — the test only asserts section content, not ordering, so it passes either way, but the code must be clean.

- [ ] **Step 2: Run the test to verify it passes**

Run: `cargo test -p wordcartel --test backlog render_is_deterministic_and_has_expected_sections -- --nocapture`
Expected: PASS.

- [ ] **Step 3: Replace the placeholder comparator with the clean real key**

```rust
    open_sorted.sort_by(|a, b| {
        b.blocks_effort_p
            .cmp(&a.blocks_effort_p) // true (blocking) first
            .then_with(|| size_rank(&a.size).cmp(&size_rank(&b.size)))
            .then_with(|| a.theme.cmp(&b.theme))
            .then_with(|| a.id.cmp(&b.id))
    });
```

- [ ] **Step 4: Re-run tests + clippy**

Run: `cargo test -p wordcartel --test backlog` then `cargo clippy -p wordcartel --tests`
Expected: PASS; clippy clean.

- [ ] **Step 5: Commit** (hold until authorized)

```bash
git add wordcartel/tests/backlog.rs
git commit  # "feat(backlog): render() dashboard generator + format pin" + trailers
```

---

## Task 3: Transcribe `backlog.toml` (real data) + real-file gate (I7 + schema)

This is the data-migration task. The transcription is verified *for completeness and validity by the gate itself* (schema validity here; the marker bijection I4 in Task 4), so the procedure below plus the spec's rename table (§9) is the authoritative instruction — the engineer does NOT need a pre-written 50-line TOML dump.

**Files:**
- Create: `backlog.toml` (repo root)
- Create: `BACKLOG.md` (repo root, via bless)
- Modify: `wordcartel/tests/backlog.rs` (add `load()` + real-file tests)

**Interfaces:**
- Consumes: `parse`, `validate`, `render`, `repo_root` (Tasks 1–2).
- Produces: `fn load() -> Manifest` reading `repo_root()/backlog.toml`.

**Transcription procedure (deterministic):**
1. **One `[[item]]` per item entry** in `docs/ux-backlog.md` and `docs/engineering-health.md`. An "item entry" is a `##`/`###`/`####` heading whose leading token is an id (e.g. `A1`, `C2b`, `H13`, `PA`, `P`) — NOT a section heading (`## Theme A …`, `## Snapshot`, `## Governing principle`, `## Resolved decisions`, `## Working order`, `## Cross-cutting notes`, `## Sizing summary`).
2. **Field extraction per entry:**
   - `id` = the leading id token.
   - `title` = the short human title (the heading's descriptive phrase up to the first ` — `/` · ` that begins a status/commit annotation). Use the concise spec titles where the header is long.
   - `status` = map the header's status word: `SHIPPED`→`shipped`; `dropped`→`dropped`; `needs-design`→`needs-design`; `settled-design`/`settled-principle`/`available-today`→`ready`; `watch`→`watch`; `triage`→`triage`; `potential-bug`→`needs-design` (with `kind = "bug"`).
   - `shipped_commit` / `shipped_date` = from the header's `(hash)` and date for shipped items (pull the exact merge hash from `git log` when the header lacks one).
   - `size` = from the "Sizing summary"/header (`Small`→`S`, `Small-Medium`→`SM`, `Medium`→`M`, `Larger`→`L`, capstone→`XL`, unknown→`TBD`).
   - `kind` = judgment: feature/bug/debt/chore/research.
   - `theme` = the item's theme letter (code-health items → `H`).
   - `blocks_effort_p` = `true` for items the docs mark "before Effort P" (`H1`/`H14`/`H9`/`H11`, the pre-P checklist items).
   - `hook` = a one-line summary (reuse the doc's one-liner).
   - `doc` = `<file>#<lowercased-id>` (e.g. `engineering-health.md#h14`).
   - `created` = first-filed date if known, else the item's earliest date in the doc.
3. **Apply the spec §9 deltas verbatim:**
   - **Split** eng-health `H1` → `H1` (`shipped`, `304e263`, `2026-07-09`) + **new `H14`** (`render()` body split, `ready`, `M`, `blocks_effort_p=true`, `depends_on=["H9","H11"]`, `doc="engineering-health.md#h14"`).
   - **Split** `A6` → `A6` (`shipped`, `2026-07-04`) + **new `A13`** (overlay mouse parity, `needs-design`, `SM`, `theme=A`).
   - **Rename** ux Theme-H `H1`→`H15` (leaf extraction, `shipped`, `2026-07-04`) and `H2`→`H16` (`active_line` clamp, `shipped`, `0573eec`, `2026-07-08`).
   - **Reconcile** `E6` → `shipped` (`242c987`, `2026-07-09`).
   - **Add** `M8` (undo louder-hint, `ready`, `S`, `theme=M`), `M9` (pulldown-cmark upgrade, `watch`, `S`, `theme=M`).
   - **Add** `P` (Effort-P capstone, `needs-design`, `XL`, `theme=P`, `doc` → `docs/design/effort-p-plugin-system-design-space.md`), `PA`/`PB`/`PC` (`research`, `theme=P`).
4. **Worked examples (copy the shape):**

```toml
[[item]]
id = "H1"
title = "God-object SEAM decomposition (app.rs/render.rs)"
status = "shipped"
kind = "debt"
size = "M"
theme = "H"
hook = "run→timers.rs SUBSYSTEMS table; reduce→10-stage Handled skeleton; leaf extractions. render() split spun out to H14."
doc = "engineering-health.md#h1"
blocks_effort_p = true
created = "2026-07-07"
shipped_commit = "304e263"
shipped_date = "2026-07-09"

[[item]]
id = "H14"
title = "Split the render() body by paint surface"
status = "ready"
kind = "debt"
size = "M"
theme = "H"
hook = "Split 522-line render() into paint_rows/paint_status/place_cursor; unify segs/placed lead-in."
doc = "engineering-health.md#h14"
blocks_effort_p = true
depends_on = ["H9", "H11"]
created = "2026-07-09"

[[item]]
id = "A13"
title = "Overlay mouse parity"
status = "needs-design"
kind = "feature"
size = "SM"
theme = "A"
hook = "Click-to-select for theme picker + file browser; outline click-to-jump."
doc = "ux-backlog.md#a13"
created = "2026-07-04"

[[item]]
id = "S3"
title = "Snapshots — durable revision checkpoints"
status = "needs-design"
kind = "feature"
size = "SM"
theme = "S"
hook = "Capture/list/diff/restore document snapshots; reuses rope snapshot + ChangeSet; one net-new display-diff."
doc = "ux-backlog.md#s3"
created = "2026-07-07"
```

- [ ] **Step 1: Author `backlog.toml`** at repo root — a leading comment banner (`# Source of truth for backlog state. Regenerate BACKLOG.md: scripts/backlog bless`) followed by every `[[item]]` per the procedure + §9 deltas above.

- [ ] **Step 2: Add `load()` + the real-file schema test** to `backlog.rs`

```rust
fn load() -> Manifest {
    let p = repo_root().join("backlog.toml");
    let s = std::fs::read_to_string(&p)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()));
    parse(&s)
}

#[test]
fn backlog_toml_is_schema_valid() {
    let errs = validate(&load());
    assert!(errs.is_empty(), "backlog.toml invariant violations:\n{}", errs.join("\n"));
}
```

(Remove the `#[allow(dead_code)]` on `repo_root` now that it is used.)

- [ ] **Step 3: Run the schema test — fix any data errors it reports**

Run: `cargo test -p wordcartel --test backlog backlog_toml_is_schema_valid -- --nocapture`
Expected: PASS (iterate on the TOML until I1–I3/I6 are clean).

- [ ] **Step 4: Add the I7 up-to-date test + generate `BACKLOG.md`**

```rust
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
```

Generate: `BLESS_BACKLOG=1 cargo test -p wordcartel --test backlog backlog_index_is_up_to_date`
Then verify: `cargo test -p wordcartel --test backlog` → all PASS.

- [ ] **Step 5: Eyeball `BACKLOG.md`** — open count is ~20, shipped log is complete, blocking-P tally correct. Confirm clippy clean: `cargo clippy -p wordcartel --tests`.

- [ ] **Step 6: Commit** (hold until authorized)

```bash
git add backlog.toml BACKLOG.md wordcartel/tests/backlog.rs
git commit  # "feat(backlog): transcribe manifest + generate dashboard + I7 gate" + trailers
```

---

## Task 4: Prose markers + status strip + Theme-H relocation (I4, I5)

**Files:**
- Modify: `docs/ux-backlog.md`, `docs/engineering-health.md`
- Modify: `wordcartel/tests/backlog.rs` (add I4 + I5 tests)

**Interfaces:**
- Consumes: `load`, `repo_root` (Task 3).

- [ ] **Step 1: Write the failing I4 + I5 tests** (append to `backlog.rs`)

```rust
const DOCS: &[&str] = &["ux-backlog.md", "engineering-health.md"];

fn read_doc(file: &str) -> String {
    std::fs::read_to_string(repo_root().join("docs").join(file))
        .unwrap_or_else(|e| panic!("cannot read docs/{file}: {e}"))
}

/// Markers of the form `<!-- item: ID -->`, one per backlog-item heading.
fn markers_in(file: &str) -> Vec<String> {
    read_doc(file)
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            t.strip_prefix("<!-- item:")
                .and_then(|r| r.strip_suffix("-->"))
                .map(|id| id.trim().to_string())
        })
        .collect()
}

fn doc_file_of(item: &Item) -> &str {
    item.doc.split('#').next().unwrap_or(&item.doc)
}

#[test]
fn i4_markers_and_manifest_are_bijective_per_file() {
    let m = load();
    for file in DOCS {
        let markers: HashSet<String> = markers_in(file).into_iter().collect();
        assert_eq!(markers.len(), markers_in(file).len(), "duplicate marker in {file}");
        let manifest_ids: HashSet<String> = m
            .item
            .iter()
            .filter(|it| doc_file_of(it) == *file)
            .map(|it| it.id.clone())
            .collect();
        let orphan_prose: Vec<_> = markers.difference(&manifest_ids).collect();
        let orphan_rows: Vec<_> = manifest_ids.difference(&markers).collect();
        assert!(orphan_prose.is_empty(), "{file}: markers with no manifest row: {orphan_prose:?}");
        assert!(orphan_rows.is_empty(), "{file}: manifest rows with no marker: {orphan_rows:?}");
    }
}

#[test]
fn i5_no_status_tokens_in_headings() {
    let banned = ["SHIPPED", " — needs-design", " — settled", " — potential-bug", " — watch", " — triage", " — dropped", " — available-today", " — fact-checked"];
    for file in DOCS {
        for (n, line) in read_doc(file).lines().enumerate() {
            if line.trim_start().starts_with('#') {
                for b in banned {
                    assert!(
                        !line.contains(b),
                        "docs/{file}:{}: status token '{b}' in heading — status belongs ONLY in backlog.toml:\n  {line}",
                        n + 1
                    );
                }
            }
        }
    }
}
```

- [ ] **Step 2: Run — verify they FAIL** (prose not migrated yet)

Run: `cargo test -p wordcartel --test backlog i4_markers i5_no_status -- --nocapture`
Expected: FAIL (missing markers; `SHIPPED` still in headers).

- [ ] **Step 3: Migrate the prose** (mechanical, per item):
  - Insert `<!-- item: ID -->` on its own line immediately under each backlog-item heading.
  - Strip the status/commit annotation from the heading text (keep the id + short title). Move any commit/date detail into the body prose if not already there (status now lives in the manifest).
  - **Relocate** the two ux Theme-H entries into `engineering-health.md` as `H15`/`H16` (heading + `<!-- item: H15 -->`/`H16` marker + their prose), and delete them from `ux-backlog.md`.
  - **Split** eng-health `H1`'s heading/prose to reflect `H1` (SEAM, shipped) and add an `H14` section (render() split) with its marker.
  - **Delete** the `## Sizing summary` and `## Working order` sections from `ux-backlog.md` (now generated).

- [ ] **Step 4: Run the full gate — all green**

Run: `cargo test -p wordcartel --test backlog`
Expected: PASS (I4 bijection holds; I5 clean). If I4 reports orphans, that is the completeness check catching a missed/extra item — reconcile TOML↔prose until bijective.

- [ ] **Step 5: Commit** (hold until authorized)

```bash
git add docs/ux-backlog.md docs/engineering-health.md wordcartel/tests/backlog.rs
git commit  # "refactor(backlog): marker prose to manifest; strip status; relocate Theme-H" + trailers
```

---

## Task 5: `scripts/backlog` wrapper + CLAUDE.md pointer + memory

**Files:**
- Create: `scripts/backlog` (`chmod +x`)
- Modify: `CLAUDE.md`
- Modify: `~/.claude/projects/-home-jkeim-projects-groundwords/memory/backlog-shorthand-bl.md`

- [ ] **Step 1: Create `scripts/backlog`**

```sh
#!/usr/bin/env sh
# Backlog helper. Source of truth: ./backlog.toml  ·  Generated view: ./BACKLOG.md
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
cmd="${1:-help}"
case "$cmd" in
  bless)
    BLESS_BACKLOG=1 cargo test -p wordcartel --test backlog backlog_index_is_up_to_date -- --nocapture
    ;;
  add)
    # Append a triage-stub item. Usage: scripts/backlog add <ID> <THEME> "<title>"
    id="${2:?usage: scripts/backlog add <ID> <THEME> \"<title>\"}"
    theme="${3:?theme required}"
    title="${4:?title required}"
    today="$(date +%Y-%m-%d)"
    {
      printf '\n[[item]]\n'
      printf 'id = "%s"\n' "$id"
      printf 'title = "%s"\n' "$title"
      printf 'status = "triage"\n'
      printf 'kind = "feature"\n'
      printf 'size = "TBD"\n'
      printf 'theme = "%s"\n' "$theme"
      printf 'hook = "%s"\n' "$title"
      printf 'doc = "ux-backlog.md#%s"\n' "$(printf '%s' "$id" | tr '[:upper:]' '[:lower:]')"
      printf 'created = "%s"\n' "$today"
    } >> backlog.toml
    echo "added $id (triage). Add a prose stub + marker, then: scripts/backlog bless"
    ;;
  open)
    sed -n '/^## Open/,/^## Shipped/p' BACKLOG.md
    ;;
  shipped)
    sed -n '/^## Shipped/,$p' BACKLOG.md
    ;;
  *)
    echo "usage: scripts/backlog {bless|add <ID> <THEME> \"<title>\"|open|shipped}"
    ;;
esac
```

- [ ] **Step 2: Make executable + smoke-test**

Run:
```bash
chmod +x scripts/backlog
scripts/backlog open      # prints the open table
scripts/backlog bless     # regenerates; must be a no-op (git diff --quiet BACKLOG.md)
git diff --quiet BACKLOG.md && echo "IDEMPOTENT OK"
```
Expected: open table prints; `IDEMPOTENT OK`.

- [ ] **Step 3: Add the CLAUDE.md pointer.** Under the top-of-file project description, add one line:

```markdown
**Backlog:** open + completed work is tracked in `backlog.toml` (source of truth) → generated
`BACKLOG.md` (read this for status). A `bl:` message files a `triage` item + prose stub and
regenerates, left uncommitted. Drift is a `cargo test` gate (`wordcartel/tests/backlog.rs`).
```

- [ ] **Step 4: Update the `bl:` memory** — rewrite `backlog-shorthand-bl.md` body to: "`bl:` files to `backlog.toml` as a `triage` item (via `scripts/backlog add` or by hand) + a prose stub under an `<!-- item: ID -->` marker in `docs/ux-backlog.md`/`engineering-health.md`, then `scripts/backlog bless`; leave uncommitted by default. Light grounding still applies." Keep the `[[backlog-shorthand-bl]]` name.

- [ ] **Step 5: Final gate sweep**

Run:
```bash
cargo test -p wordcartel --test backlog
cargo clippy --workspace --all-targets
```
Expected: all PASS; clippy clean.

- [ ] **Step 6: Commit** (hold until authorized)

```bash
git add scripts/backlog CLAUDE.md
git commit  # "feat(backlog): scripts/backlog wrapper + CLAUDE.md pointer" + trailers
```

---

## Self-Review

**Spec coverage:**
- Schema §4 → Task 1 (types/enums/validate), Task 3 (real data). ✔
- Both files at root §3 → Global Constraints + `repo_root()` (Task 1) + Task 3. ✔
- Tooling / gate §5 → Tasks 1–4 (`backlog.rs`), Task 5 (`scripts/backlog`). ✔
- `bl:` §6 → Task 5 (`add` + memory). ✔
- Invariants I1–I7 §7 → I1–I3/I6 Task 1; I7 Task 3; I4/I5 Task 4. ✔
- Dashboard format §8 → Task 2 `render()`. ✔
- Rename/merge/reconcile §9 → Task 3 Step (§9 deltas) + Task 4 relocation. ✔
- Scope boundaries §10 → nothing touches CHANGELOG/SDD/specs/memory-decisions (only the `bl:` memory + CLAUDE.md pointer). ✔
- Migration phases §11 → Tasks 3 (author+generate), 4 (prose), 5 (wrapper/docs); Phase-2 archival deliberately omitted (deferred). ✔

**Placeholder scan:** The Task 2 sort comparator ships a deliberate placeholder in Step 1 that Step 3 replaces with the clean key — flagged inline, not a hidden TODO. Task 3 is a data-transcription task whose completeness is enforced by the I4 bijection gate rather than a pre-written 50-item dump; the extraction rules + §9 table + worked examples are complete. No other placeholders.

**Type consistency:** `Manifest`/`Item` fields, `validate`/`render`/`parse`/`load`/`repo_root`/`markers_in`/`doc_file_of` signatures are consistent across Tasks 1–4. `BLESS_BACKLOG` env var and the regenerate command string match between the test, the wrapper, and CLAUDE.md.
