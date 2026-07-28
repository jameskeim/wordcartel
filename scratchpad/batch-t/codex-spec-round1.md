The spec is not ready to advance to planning. The H38 and H28 designs are source-grounded, but H36’s governing count is wrong and two verification instruments contain material contradictions.

## Critical

### C1 — The “98-site sweep” is arithmetically impossible

**Spec claim**

> “Of the 116: **98 sweepable `.join` constructions** (96 same-line + 2 line-wrapped), 12 bare code sites, 3 comment lines, 1 seam impl, 2 out-of-crate.”  
> — spec lines 50–56

and:

> “D4: the **full 98-site sweep** … [excluding] `swap.rs:48`, the 2 integration-test sites, the seam’s own impl.”  
> — spec line 67

**Real source**

The tree contains:

- 116 total `temp_dir()` hits.
- 3 comment-only hits:
  - `wordcartel/src/prompts.rs:487`
  - `wordcartel/src/prompts.rs:717`
  - `wordcartel/src/swap.rs:675`
- 12 bare code uses.
- 101 `.join` constructions:
  - 99 same-line `temp_dir().join`
  - 2 wrapped joins at `wordcartel/src/prompts.rs:1263` and `:1383`.

Of those 101 joins, four are binding exclusions:

- `wordcartel/src/swap.rs:48`
- `wordcartel/src/test_support.rs:294`
- `wordcartel/tests/harper_ls_probe.rs:44`
- `wordcartel/tests/harper_ls_integration.rs:57`

Therefore the sweepable population is:

`101 − 4 = 97`, not 98.

The correct complete partition is:

`97 swept joins + 4 excluded joins + 12 bare code uses + 3 comments = 116`.

The spec’s “96 same-line + 2 wrapped” appears to exclude the seam and two integrations but incorrectly includes `swap.rs:48`, even though D4 explicitly excludes that site.

**Why it matters**

This directly breaks D4’s binding scope, R8, §10.3’s deviation arithmetic, and the archive figures in §12. An implementer cannot both sweep 98 sites and preserve every named exclusion. Planning against this number would force either an overreach into `swap::state_dir()` or a false incomplete-sweep report.

This is blocking.

---

## Important

### I1 — The claimed file count is wrong

**Spec claim**

> “the population is **116 grep hits across 28 files**”  
> — spec line 50

**Real source**

`temp_dir()` occurs in 29 Rust files:

- 27 files under `wordcartel/src`
- 2 files under `wordcartel/tests`

The per-file list itself implicitly exposes the mismatch: the eleven explicitly counted files, twelve other files with 1–2 hits, `test_support.rs`, and two integration files total at least 26 categories/files, while the complete real inventory totals 29.

**Why it matters**

This is part of the corrected H36 record that §12 says must be archived as authoritative. Shipping another incorrect census defeats that correction. It also makes “26-file mechanical series” ambiguous: 26 is the number of source files actually affected by the mechanical sweep, while 29 is the full hit inventory.

---

### I2 — §10.5’s region-check algorithm cannot handle `e2e.rs`

**Spec claim**

> “for every sweep commit, every diff hunk in every touched file falls at/after that file’s `#[cfg(test)] mod tests` line (grounding verified all 98 sites sit in trailing test mods)”  
> — spec lines 446–449

**Real source**

The sweep includes a construction in:

- `wordcartel/src/e2e.rs:3087`

But that file has no `#[cfg(test)] mod tests` marker. It is test-gated at file level:

- `wordcartel/src/e2e.rs:1`: `#![cfg(test)]`
- `wordcartel/src/lib.rs:86`: `#[cfg(test)] mod e2e;`

Likewise, the source assertion that all sites sit in “trailing test mods” is false for `e2e.rs`.

**Why it matters**

The promised mechanical premise audit either fails a legitimate sweep commit or silently skips a touched file. Since §10.5 is the principal proof that the sweep cannot reach shipped code, its script specification must explicitly support both:

- trailing `#[cfg(test)] mod tests` regions; and
- whole-file test modules such as `e2e.rs`.

This does not invalidate the shipped-behavior premise itself; it invalidates the proposed proof of that premise.

---

### I3 — §10.1 claims four new test names, but T2 is not a new test

**Spec claim**

> “For the whole branch: the only inventory delta versus base is the four named additions T1–T4”  
> — spec lines 386–389

**Real source**

T2 extends the existing test:

- `wordcartel/src/ltex_ls.rs:151`: `classify_maps_languagetool_speller_rules_to_spelling`

It adds two assertions but no test name. The actual new test inventory is three names:

- T1: new direct heuristic test
- T3: new agreement test
- T4: new destination-kind test

The spec itself correctly says:

> “Three tests are added”  
> — spec line 80

**Why it matters**

The baseline-to-tip inventory oracle is specified with the wrong expected delta. A correct run would show three new names and contradict §10.1. Rewrite it to distinguish “four test tasks/instruments” from “three newly registered tests plus two assertions added to one existing test.”

---

### I4 — The residue oracle is correct, but it cannot prove “98 swept”

**Spec claim**

> “The residue oracle … proves completeness AND exclusion-compliance at once”  
> — spec lines 399–419

and R8:

> “All 98 join sites swept … unswept site → extra grep hit”  
> — spec line 489

**Real source**

After D5 removes its three bare seeds and the actual 97 sweepable joins are rewritten, the exact code survivor list is indeed 13 hits:

- 9 remaining bare sites
- `swap.rs:48`
- `test_support.rs:294`
- 2 integration-test joins

That matches the categories in §10.2.

But the oracle proves that all **97** non-excluded joins disappeared. It cannot prove 98 were swept because preserving the exact survivor list necessarily preserves `swap.rs:48`.

**Why it matters**

The survivor list itself is sound; the interpretation and carriage claim are not. R8, §10.3’s `98 sites − template count`, and §12’s “98 swept” must all be corrected together.

---

## Minor

### M1 — The LanguageTool evidence is slightly overstated

**Spec claim**

> “Grounding §1.1 proved the branch LIVE”  
> — spec lines 32–35

and:

> “the branch is reachable in plain en-US usage”  
> — spec lines 106–107

**Real supporting evidence**

The real classifier logic is exactly as claimed:

- `wordcartel/src/ltex_ls.rs:84–90` uppercases the code and short-circuits only on `MORFOLOGIK`, `HUNSPELL`, or `SPELLER`.
- Both `FR_SPELLING_RULE` and `EN_CONTRACTION_SPELLING` contain `SPELL` and contain none of those three full substrings.
- `wordcartel/src/lsp_client.rs:64–75` then classifies them through lowercase `contains("spell")`.
- With `"message":"x"`, deleting that branch yields `Grammar`.

The external grounding verifies those identifiers in the installed LanguageTool 6.8 jars, but explicitly says the propagation of the LanguageTool rule ID into LSP diagnostic `code` was inferred and not live-probed:

- `scratchpad/batch-t/fable-grounding.md:37–41`

**Why it matters**

The proposed tests and kill conditions remain valid. The prose should say “jar-grounded real rule IDs, with LSP-code propagation inferred from the existing protocol and probe history,” rather than claiming fully demonstrated runtime reachability.

---

## Clean sections

- **H38 source structure and kill conditions:** Clean. The signatures, visibility, duplicate bodies, current coverage hole, short-circuit behavior, and proposed mutation effects all match `lsp_client.rs:59–77`, `ltex_ls.rs:81–90,151–160`, and `harper_ls.rs:156–172,528–534`.
- **H28 reachability:** Clean. Production entry points seed empty fields (`prompts.rs:83–93`, `blocks_marked.rs:127–143`), the picker starts with empty entries and asynchronously lists (`editor.rs:1042–1063`), Enter has no listing guard (`file_browser_intercept.rs:59–67`), and `Nothing` produces the stated Sticky Warning and aborts the Save-As quit drain (`file_browser_commit.rs:353–377`).
- **T4 mechanism:** Clean. `EntryKind` has exactly `File`, `Dir`, `Other`, and `Unknown` (`fsx.rs:48–57`); Row 1 handles directories, Row 2 handles only files, and `Other`/`Unknown` fall to `Nothing` (`file_browser_commit.rs:92–114`). The named widening mutation discriminates T4 from the existing `None` test at `:2275–2283`.
- **Route B listing plausibility:** Clean. `Other` comes from neither-file-nor-directory entries and broken symlinks become `Unknown` (`fsx.rs:259–275`); broken entries bypass type filtering and extensionless names pass `is_document` (`file_browser_listing.rs:103–110,127–143`).
- **Scratch seam and `swap::state_dir()`:** Clean. The seam is pid-plus-atomic-sequence based and `scratch_dir` creates the directory (`test_support.rs:287–319`). `state_dir()` is a stable per-process test redirect and must not use the counter seam (`swap.rs:46–53`).
- **Shipped-behavior premise:** Substantively clean. All proposed executable changes are in test-only regions; D5 changes test fixtures only; H28 changes test comments; the sole production-region touch is the prose-only doc comment at `lsp_client.rs:59–62`. The §10.5 audit implementation needs repair, but the premise itself holds.
- **Command-surface contract:** Correctly N/A. The contract applies to commands, settings/options, palette, menu, keybindings, and hints (`command-surface-contract.md:3–10`). Batch T touches none of them.
- **Carriage R1–R7 and R9–R13:** Mechanistically credible apart from the count-dependent wording and §10.5 defect described above. R8 is unsound as written because the target is 97, not 98.

The spec needs a coordinated correction of every 98-site assertion, the file count, the inventory expectation, the residue-oracle interpretation, the archive figures, and the whole-file `cfg(test)` handling before planning.

**VERDICT: NOT READY**

Codex session ID: 019fa5a3-d2e8-7622-8f6c-778a82edff95
Resume in Codex: codex resume 019fa5a3-d2e8-7622-8f6c-778a82edff95
