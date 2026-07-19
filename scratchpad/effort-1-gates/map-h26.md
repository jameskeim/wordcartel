# H26 — fs_chokepoint scanner: structure and evasion surface

Mapped 2026-07-19 by a read-only agent instructed to report facts only, no recommendations.
Transcribed by the controller (the agent had no write access).

## Structure

`wordcartel/tests/fs_chokepoint.rs`, 568 lines. **4 layers**, each run over `strip_test_modules`
output (cuts at the first `#[cfg(test)]` + `mod tests` PAIR, line-pair lookahead only — not a bare
`#[cfg(test)]` attribute).

- **L1** `has_std_fs_import` — `use std::fs` prefix, or `use std::{…fs…}`.
- **L2** textual `t.contains("std::fs::")`.
- **L3** `PATH_FS_METHODS`, a closed list — `metadata, symlink_metadata, canonicalize, read_link,
  read_dir, exists, try_exists, is_file, is_dir, is_symlink` — matched as `.method(` or
  `Path::method(`.
- **L4** `wrapper_offenders_in` — derives in-crate wrappers via `pins_realfs` (contains
  `fsx::RealFs`, no `Arc::new(`), attributes each to its enclosing top-level `fn`
  (`top_level_fn_name`, column-0 only), then flags production callers matching
  `module::name(` for any derived wrapper, plus any inline `RealFs` pin.

Excludes `src/e2e.rs` and `src/test_support.rs`. `EXEMPT_MODULES` has exactly ONE entry:
`src/fsx.rs` (clause d, the seam's own impl).

## Markers

Format `// fs-chokepoint-allow: (x) reason`, `x ∈ a..g`, validated by exact position
(`r[0]=='('`, `r[2]==')'`, `r[1] in a..=g`). L4 additionally accepts `(w) <non-empty prose>`.
**Clause letter meanings are NOT defined in the scanner** — it enforces letter + count only;
semantics live in the C5 spec §2.3. A marker must sit on the offending line or the line directly
above.

**Current counts in `wordcartel/src`: `(w)`=37, `(c)`=6, `(b)`=6, `(g)`=1, `(e)`=1 — total 51.**
No `(a)`, `(d)`, or `(f)` markers exist.

## Evasion surface (item 3) — 5 of 6 NOT caught

| Technique | Caught? | What the scanner sees |
|---|---|---|
| `use std::fs as f;` then `f::read(…)` | **yes** | L1 matches the import line regardless of alias |
| Same-module unqualified wrapper call (bare `open(p)` inside `file.rs`) | **no** | `wrapper_offenders_in` explicitly skips `w.module == module` — a *disclosed* gap, commented in the source as such |
| Renamed / re-exported wrapper (`pub use file::open as fetch;` then `fetch(p)`) | **no** | matches literal `"{module}::{name}("`; a re-export has neither |
| Function-pointer / closure indirection (`let f = file::open; f(p)`) | **no** | no textual `module::name(` at the use site |
| `RealFs` unqualified after `use crate::fsx::RealFs;` | **no** | `pins_realfs` requires the literal substring `fsx::RealFs` |
| `(w)` marker with vacuous or wrong prose | **no** | only checks the trailing text is non-empty; no semantic check |

## The 15 RealFs-pinning wrappers

All have an fs-taking sibling. `file::open`→`open_with_fs`; `file::bounded_read_opt`;
`file::save_atomic`; `file::save_atomic_bytes`; `config::config_layer_paths`; `config::load`;
`state::load_in`; `state::file_identity`; `save::fingerprint`; `swap::delete`;
`swap::find_orphan_scratch_swap`→**`find_orphan_scratch_swap_in`** (sibling is `_in`, NOT
`_with_fs` — a scan for the literal `_with_fs` would miss this pairing); `swap::write_atomic`;
`theme_resolve::resolve_theme`; `diagnostics_run::append_word_to_dict`; `plugin::load::discover`.

## Dependency availability (item 5) — decision-relevant

`wordcartel`'s `[dev-dependencies]` are **`proptest` and `tempfile` only**. No `syn`,
`proc-macro2`, `ra_ap_*`, or `tree-sitter` is a dependency of ANY workspace member. `syn 2.0.118`
and `proc-macro2` appear in `Cargo.lock` only as transitive deps of unrelated packages
(`bindgen`, `burn-derive`) not reachable from this workspace's members. **Use-tree parsing would
require adding a new declared dependency** — against a project with a stated dependency-weight
concern (H2).

## Gate invocation

**No CI exists** — no `.github/workflows`, no pre-commit hook beyond the unmodified sample, no
gate script referencing `fs_chokepoint`. It is an ordinary integration test with no `#[ignore]` or
feature gate, so it runs under plain `cargo test`. Measured: 0.12s harness, 0.44s wall post-build,
10 tests pass. The docs call it a "merge gate" descriptively; **no separate enforcement mechanism
exists in the repo.**

## Self-checks

7 tests, including `scanner_detects_every_evasion_route` (plants one case per L1/L2/L3 route),
`scanner_sees_production_code_below_an_early_cfg_test_attribute` (bidirectional, cites the real
`app.rs` incident), `layer_four_detects_a_wrapper_call_that_layers_one_to_three_are_blind_to`,
`layer_four_wrapper_derivation_excludes_an_arc_composition_root`,
`layer_four_markers_must_carry_a_reason`, `a_per_hit_marker_exempts_only_its_own_line`,
`a_marker_without_a_clause_is_not_an_exemption`.

**None of the self-checks cover any item-3 evasion above**, nor the three disclosed
import-spelling gaps (nested-group, renamed-in-group, leading-root `::std::fs`). Those are stated
as accepted, unproven gaps in the file header and in the H26 backlog entry.
