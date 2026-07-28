The plan is not ready to execute. The source-level snippets and the 97-site census are sound, but the mutation workflow and several required verification instruments are incomplete or incapable of returning failure.

## Critical

### 1. The mutation “clean tree” requirement is impossible at the stated point in both TDD tasks

Plan:

> “Restore; `git diff` clean.”  
> — plan:206–207

and:

> “Restore; `git diff` clean. Record both observations.”  
> — plan:281–282

At these points, the new tests and comment changes have already been written but are not committed until Task 1f or Task 2e. Those intended changes necessarily remain in `git diff`. The requested clean result therefore cannot occur unless the implementer silently stages or commits the intended changes before mutation, neither of which the plan instructs.

The real tree at `73b8ca2` contains only the existing Harper classification test at [harper_ls.rs:529](/home/jkeim/projects/groundwords/wordcartel/src/harper_ls.rs:529), the existing LTeX test at [ltex_ls.rs:151](/home/jkeim/projects/groundwords/wordcartel/src/ltex_ls.rs:151), and the existing no-highlight picker test at [file_browser_commit.rs:2276](/home/jkeim/projects/groundwords/wordcartel/src/file_browser_commit.rs:2276). Thus all proposed new tests will be outstanding modifications during their mutation steps.

This is not cosmetic: restoration is a binding part of the approved mutation protocol, and the stated verification cannot pass. The plan must prescribe a workable baseline, such as staging the intended task diff and checking both `git diff` and `git diff --cached` against recorded baselines, or committing the pin before performing a temporary mutation.

### 2. The delegation ledger reports violations but always exits successfully

Plan:

> `[ $((t-b)) -eq "$n" ] && echo "OK …" || echo "LEDGER FAIL …"`  
> — plan:520–523

and:

> `[ "$b" = "$t" ] || echo "UNEXPECTED seam-call delta …"`  
> — plan:525–530

Neither loop accumulates failure nor ends with a failing assertion. A short delta, excess delta, or unexpected seam call merely prints text; the script’s final command can still return zero. That contradicts the spec’s requirement:

> “Require `S_tip(f) − S_base(f) == E(f)` for every file in the tree.”  
> — spec:10.2

This matters because the ledger is the only mechanical instrument distinguishing actual delegation from deletion or replacement with a different constructor. As written, the required verification can fail semantically while succeeding as a command.

### 3. The per-commit region check also always exits successfully

Plan:

> `echo "FAIL $c: …"`  
> …  
> `done; echo "REGION CHECK: done"`  
> — plan:536–546

All three violation paths only print `FAIL`; the unconditional final `echo` forces a successful exit. A production-region hunk, a non-Rust file, or a file matching neither test-region shape therefore does not reject the sweep.

The real tree contains both shapes the check must protect: ordinary files use trailing test modules, e.g. [file_browser_commit.rs:515](/home/jkeim/projects/groundwords/wordcartel/src/file_browser_commit.rs:515), while [e2e.rs:1](/home/jkeim/projects/groundwords/wordcartel/src/e2e.rs:1) is whole-file `#![cfg(test)]`. The approved spec explicitly says a violation means “the commit is rejected” (§10.5). This command cannot enforce that requirement and therefore cannot protect the batch’s shipped-behavior premise.

### 4. The required keyed-digest sample is not implemented

Plan:

> `# Sample = ALL fmt==1 candidates … + the 18 lowest-ranked others, + floor top-ups …`  
> — plan:571–573

This is only a comment. There is no shell that produces the selected set, removes duplicates, applies the eleven heavy-file top-ups in pinned path-byte order, or enforces the three `scratch_path` and three `scratch_dir` floors.

The approved spec makes that exact selection algorithm binding (§10.3): unconditional dynamic-label inclusions, an 18-candidate digest draw, then ordered file and seam-kind top-ups. The plan instead leaves the implementer to select the sample manually. Consequently two reviewers cannot mechanically obtain the promised identical set, and the principal review instrument for wrong F/D choices and wrong labels does not exist.

This is especially serious because the source contains the dynamic wrapped sites that unconditional inclusion is meant to protect, at [prompts.rs:1263](/home/jkeim/projects/groundwords/wordcartel/src/prompts.rs:1263) and [prompts.rs:1383](/home/jkeim/projects/groundwords/wordcartel/src/prompts.rs:1383).

## Important

### 5. The sweep tasks do not carry complete per-site implementation code or even a per-site F/D decision

Plan:

> “F-vs-D decision: does the TEST treat the path as a file … or as a directory …?”  
> — plan:417–420

Tasks 4–8 then provide only filenames and counts, for example:

> “`app.rs` (11) → `jobs_apply.rs` (9) → `render_overlays.rs` (7)”  
> — plan:440–441

Project law requires implementation plans to contain “COMPLETE code” and grounded migration sites. The plan supplies only two generic templates, not the 97 concrete replacements, labels, or provisioning deletions. The implementer is left to re-decide:

- `scratch_path` versus `scratch_dir`;
- the exact normalized label;
- which adjacent provisioning lines belong to the construction;
- whether a dynamic discriminator must remain in `format!`.

These decisions are not uniformly trivial in the real source. Examples include adjacent constructions at [jobs_apply.rs:1000](/home/jkeim/projects/groundwords/wordcartel/src/jobs_apply.rs:1000), wrapped constructions at [prompts.rs:1263](/home/jkeim/projects/groundwords/wordcartel/src/prompts.rs:1263), and directory provisioning at [recovery.rs:147](/home/jkeim/projects/groundwords/wordcartel/src/recovery.rs:147). The approved spec explicitly identifies wrong F/D selection and dropped dynamic discriminators as residual risks. A complete plan needs a 97-row mapping or complete per-file diffs.

### 6. The branch-level inventory assertion has no executable procedure

Plan:

> “Branch-level inventory delta versus BASE … exactly three new names”  
> — plan:479–482

Unlike the pre-sweep/tip comparison, no command identifies the branch point, captures the base inventory, normalizes it, or asserts the three-name delta. This is a binding §10.1 claim, not an optional narrative check. The task leaves the reviewer to invent the procedure and could miss a renamed or removed pre-existing test elsewhere in Tasks 1–3.

### 7. Several “MUST” checks print expected values without asserting them

The residue oracle ends in:

> `wc -l    # MUST print 13`  
> — plan:487–489

and the inventory step uses:

> `diff … && echo "INVENTORY: identical"`  
> — plan:475

The inventory command does return nonzero on a diff, but the residue command always succeeds after printing a number. The subsequent anchor verification is prose rather than an executable assertion. Given the task’s explicit requirement that verification instruments genuinely detect their claims, the residue oracle should fail unless the count is 13 and the anchored survivor set is exact.

## Minor

### 8. The worktree cleanup command is invalid

Plan:

> `git worktree remove /tmp/bt-pre /tmp/bt-tip`  
> — plan:588–589

`git worktree remove` accepts one worktree path per invocation:

```text
usage: git worktree remove [-f] <worktree>
```

The cleanup must be two commands. This does not compromise the verification result, but it leaves both temporary worktrees registered.

## Cross-checks that passed

- The proposed T1–T4 Rust snippets compile in principle against the real signatures, visibility, imports, enum variants, and helper shapes.
- `classify_destination_enter` has exactly the five arguments used by T4 at [file_browser_commit.rs:77](/home/jkeim/projects/groundwords/wordcartel/src/file_browser_commit.rs:77).
- `FileEntry` has the expected `name`, `kind`, `is_symlink`, and `broken` fields at [file_browser.rs:8](/home/jkeim/projects/groundwords/wordcartel/src/file_browser.rs:8); the existing `fe(name, kind)` helper constructs all four at [file_browser_commit.rs:530](/home/jkeim/projects/groundwords/wordcartel/src/file_browser_commit.rs:530).
- `EntryKind` is `Clone + Copy + Debug + PartialEq + Eq` with exactly `File`, `Dir`, `Other`, and `Unknown` at [fsx.rs:48](/home/jkeim/projects/groundwords/wordcartel/src/fsx.rs:48).
- The named T1 and T2 mutations genuinely redden their intended new tests. The Harper negative control also reddens the existing classification test.
- `scratch_path` and `scratch_dir` accept `&str` and return `PathBuf` at [test_support.rs:305](/home/jkeim/projects/groundwords/wordcartel/src/test_support.rs:305) and [test_support.rs:316](/home/jkeim/projects/groundwords/wordcartel/src/test_support.rs:316).
- Independent source counting confirms 97 sweepable joins across the stated 25 files, with per-file counts matching the plan. The 12-commit grouping covers each swept file once and preserves every binding exclusion.
- The internal order H38 → H28 → D5 → sweep is correct, and no file is split across sweep commits.

**VERDICT: NOT READY**

Codex session ID: 019fa5df-51d4-75f2-bc4c-7f85e7e4be76
Resume in Codex: codex resume 019fa5df-51d4-75f2-bc4c-7f85e7e4be76
