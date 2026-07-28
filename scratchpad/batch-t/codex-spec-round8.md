No Critical findings. Two Important findings remain.

## Important

### I-1 — `<pre-sweep>` is defined as two different commits

The spec says:

> “`<pre-sweep>` is the commit immediately after D5's commit — the effort's last non-sweep commit”  
> — [spec:540–542](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:540)

Those descriptions are mutually exclusive:

- The commit immediately after D5 is the first sweep commit.
- The effort’s last non-sweep commit is D5 itself.

For `git diff <pre-sweep>..<tip>` to contain the entire sweep, `<pre-sweep>` must be the D5 commit—the commit immediately before the first sweep commit. This also agrees with §10.1’s explicit baseline:

> “the commit BEFORE the first sweep commit”  
> — [spec:435–437](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:435)

and §10.2:

> “the commit BEFORE the first sweep commit (so D5's three added `scratch_dir` seeds … are inside the baseline)”  
> — [spec:487–490](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:487)

Why it matters: taken literally, the new definition excludes the first sweep commit from the aggregate diff, read-set candidates, and `S_tip−S_base`. The ledger would then fail, but the review procedure would be internally impossible rather than executable.

Endpoint movement is also incompletely specified. The spec says a non-sweep fix may land “AFTER the tip, and the endpoints move accordingly” ([spec:545–548](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:545)), but advancing the tip across that fix would immediately violate the sweep-only interval. Such a fix must either remain outside the sweep endpoint or be reordered before D5/the corrected pre-sweep endpoint.

For `S_base`, the intended behavior is otherwise recoverable: `E(f)` remains the base-tree census ([spec:483–486](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:483)), while `S_base` must be remeasured at the recomputed pre-sweep commit ([spec:487–490](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:487). The spec should say this explicitly while correcting the endpoint identity.

The re-run enumeration also omits the tip-only residue oracle. It enumerates inventory/green endpoints, ledger endpoints, and the read set ([spec:548–550](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:548)), but the survivor oracle is likewise evaluated “At the sweep tip” ([spec:456–460](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:456)) and must be rerun when that endpoint changes.

### I-2 — exclusion integrity is still claimed more strongly than the instruments establish

The binding requirement is full line integrity:

> “The exclusion list … these lines do not change in the sweep”  
> — [spec:299–301](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:299)

But the oracle only checks that the expected `temp_dir()` occurrences remain:

> “Anchor survivors by symbol/context … A missing survivor = the sweep over-reached”  
> — [spec:472–474](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:472)

The spec then overstates that evidence:

> “the oracle proves the 97 non-excluded joins are GONE and the exclusions INTACT”  
> — [spec:477–479](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:477)

and repeats it in the §10.3 anchor:

> “zero non-excluded `temp_dir()` joins survive and every exclusion is intact”  
> — [spec:519–523](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:519)

R9 is stronger still:

> “Exclusions untouched … missing survivor → oracle fails”  
> — [spec:702](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:702)

A modification can retain the `temp_dir()` token and therefore pass the oracle. For example, the H28 survivor line contains both the seed and other arguments ([prompts.rs:500–504](/home/jkeim/projects/groundwords/wordcartel/src/prompts.rs:500)); changing another part of that line leaves the expected grep survivor present. A mixed, template-sized adjacent edit can also evade mandatory-read triggers under the document’s own §10.3 residual model.

Why it matters: this is the requested weakest-statement failure. The guaranteed property is “every expected excluded `temp_dir()` occurrence survives,” not “the excluded lines are untouched.” Full untouched-line integrity is only per-task/human review unless a base-to-tip content comparison for those exact sites is added.

## Prior findings

- Prior I-1: **partially fixed**. The sweep-only rule now correctly forbids every non-sweep change in the interval ([spec:540–546](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:540)), including the H28/D5 overlap files. The replacement endpoint definition is internally contradictory, however, so the interval rule is not yet fully implementable.
- Prior I-2: **fixed**. §10.2 now limits guaranteed pure-hunk detection to halves landing in separate hunks and assigns coalesced halves to sampling/per-task review ([spec:497–502](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:497)). §10.3 says the same, expressly citing adjacent `jobs_apply.rs:1000–1001` ([spec:561–569](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:561)); those constructions are genuinely adjacent in source ([jobs_apply.rs:995–1005](/home/jkeim/projects/groundwords/wordcartel/src/jobs_apply.rs:995)). R8 is aligned as well ([spec:701](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:701)).

## Weakest-statement audit

Apart from exclusion integrity, the revised sweep claims consistently use §10.3’s two strengths:

- Pure or oversized hunks are guaranteed reads.
- Template-shaped unsampled hunks receive only probabilistic sampling plus per-task review.

That qualification is now consistent in §5.2 ([spec:278–280](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:278)), invariant 4 ([spec:353–357](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:353)), §9 ([spec:399–410](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:399)), the §10.3 anchor ([spec:519–534](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:519)), R8, and R12 ([spec:701–705](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:701)). I found no renewed overclaim about F-vs-D choice, bound-name use, labels, compensating pairs, or scanner detection.

## Regression scan

The substantive design remains grounded:

- Census: 116 hits in 29 files; 99 same-line joins plus the two wrapped joins at [prompts.rs:1263](/home/jkeim/projects/groundwords/wordcartel/src/prompts.rs:1263) and [prompts.rs:1383](/home/jkeim/projects/groundwords/wordcartel/src/prompts.rs:1383), yielding 101 joins and 97 after the four exclusions. The spec’s partition and per-file ledger remain consistent ([spec:55–79](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:55)).
- Exclusions and 13 post-batch survivors remain correctly enumerated ([spec:299–311](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:299), [spec:458–470](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:458)).
- The scratch seam is pid+sequence based and `scratch_dir` provisions the directory as claimed ([test_support.rs:287–319](/home/jkeim/projects/groundwords/wordcartel/src/test_support.rs:287)).
- H38’s branch and engine fall-through pins match the real classifier paths ([lsp_client.rs:59–76](/home/jkeim/projects/groundwords/wordcartel/src/lsp_client.rs:59), [ltex_ls.rs:81–91](/home/jkeim/projects/groundwords/wordcartel/src/ltex_ls.rs:81), [harper_ls.rs:156–171](/home/jkeim/projects/groundwords/wordcartel/src/harper_ls.rs:156)).
- H28 Route B matches the real exhaustive Row-1/Row-2 distinction ([file_browser_commit.rs:77–105](/home/jkeim/projects/groundwords/wordcartel/src/file_browser_commit.rs:77)); the existing test only pins the `None` case ([file_browser_commit.rs:2275–2283](/home/jkeim/projects/groundwords/wordcartel/src/file_browser_commit.rs:2275)).
- D5’s three pumped sites and absolute-path reasoning remain present at [prompts.rs:702–724](/home/jkeim/projects/groundwords/wordcartel/src/prompts.rs:702), [prompts.rs:778–802](/home/jkeim/projects/groundwords/wordcartel/src/prompts.rs:778), and [prompts.rs:805–823](/home/jkeim/projects/groundwords/wordcartel/src/prompts.rs:805).
- The command-surface N/A conclusion remains coherent: the declared scope is test code/comments plus one production doc comment ([spec:363–374](/home/jkeim/projects/groundwords/docs/superpowers/specs/2026-07-27-batch-t-test-integrity-design.md:363)), outside the contract’s command/option/palette/menu/keybinding scope ([command-surface-contract.md:3–10](/home/jkeim/projects/groundwords/docs/design/command-surface-contract.md:3)).

No other new Critical or Important substantive regressions were found. No Cargo, build, or test command was run.

NOT READY

Codex session ID: 019fa5cf-1d8f-7ca3-b591-c722b09bf825
Resume in Codex: codex resume 019fa5cf-1d8f-7ca3-b591-c722b09bf825
