# Task 4 — guard-preservation mutation log

Purpose: prove the two `[files]` assertions added by `ea01138` still bear load after
`be22a1d` ("fix(h31): unique scratch path per call; fold three identical config test
helpers") folded the three scratch-path helpers into `load_cfg`. Each mutation changes exactly one
property; the required outcome names exactly one assertion that must fail (§7.0 of
`DISPATCH-CONSTRAINTS.md`). Each mutation is reverted and the reversion is proven
byte-for-byte with `git diff --exit-code` before the next mutation is applied.

Starting HEAD for this task: `a9b3b6d`.

## Mutation (a) — the warning guard

**Target.** `load_with_fs`'s `raw.files.type_filter` match, the `other =>` arm only
(wordcartel/src/config.rs, `[files]` block). Changed the arm from pushing a warning to a
compiling no-op:

```diff
-                other => warns.push(format!("files.type_filter \"{other}\" invalid; using documents")),
+                other => { let _ = other; },
```

Nothing else touched — not the `"documents"` arm, not `FilesConfig::default()`.

**Command:**
```zsh
cargo test -p wordcartel --lib files_type_filter_unknown_warns_and_defaults_documents
```

**Result:** compiled clean (`Finished \`test\` profile ... in 2.97s` — a genuine
recompile, not cached), then failed. Verbatim panic:

```
thread 'config::tests::files_type_filter_unknown_warns_and_defaults_documents' (2643330) panicked at wordcartel/src/config.rs:1364:9:
the invalid-value arm must warn by name (H31 diagnostic); warns was: []
```

This is `config.rs:1364:9` — the `warns.iter().any(...)` assertion with Task 1's custom
message — and NOT `config.rs:1363` (the preceding `assert_eq!(cfg.files.type_filter, ...)`
above it, which still passed since the mutation never touches `type_filter`
assignment). This confirms the mutation was confined to the warning arm as required:
the one named assertion failed, and only that one.

**Revert:** restored the `warns.push(...)` line verbatim.

**Byte-for-byte reversion check:**
```zsh
git diff --exit-code -- wordcartel/src/config.rs; rc=$?
```
`rc=0`.

## Mutation (b) — the default-on-absent guard

**Target.** `FilesConfig`'s `Default` impl, `type_filter` field only; `show_clutter`
left at `false`:

```diff
-        FilesConfig { show_clutter: false, type_filter: FileTypeFilter::Documents }
+        FilesConfig { show_clutter: false, type_filter: FileTypeFilter::All }
```

**Command:**
```zsh
cargo test -p wordcartel --lib files_filters_default_on_absent
```

**Result:** compiled clean (`Finished \`test\` profile ... in 2.65s` — genuine recompile),
then failed. Verbatim panic:

```
thread 'config::tests::files_filters_default_on_absent' (2643792) panicked at wordcartel/src/config.rs:1356:9:
assertion `left == right` failed: files.type_filter must default to Documents
  left: All
 right: Documents
```

This is `config.rs:1356:9` — the `type_filter` assertion with its own message
`"files.type_filter must default to Documents"` — and NOT the earlier
`show_clutter` assertion (`config.rs:1354`), which still passed since `show_clutter`
was left at `false`. Confirms the mutation and the asserted property are one-to-one,
as required.

**Revert:** restored `type_filter: FileTypeFilter::Documents` verbatim.

**Byte-for-byte reversion check:**
```zsh
git diff --exit-code -- wordcartel/src/config.rs; rc=$?
```
`rc=0`.

## Tree-restored confirmation (step 5)

```zsh
git status --short     # empty
git diff                # empty
cargo test               # full workspace suite
```
`cargo test` exit code `0`. `wordcartel` lib suite:
`test result: ok. 1777 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out` —
matches the expected total (1777 passing, 1 ignored) named in the dispatch. All other
workspace test binaries (`wordcartel-core`, `wordcartel-nlp`, integration/oracle
suites) also reported `ok` with 0 failed.

## Final gates (step 6)

Forced recompile via `touch wordcartel/src/config.rs` before each of build and clippy
(both showed `Compiling`/`Checking wordcartel v0.5.0`, not a cached instant return —
build 1.33s, clippy 3.65s, test --no-run 2.44s, all plausible for a real single-crate
recompile, not the "0.18s cached" red flag named in the standing constraints).

```zsh
L=$(mktemp -d)
touch wordcartel/src/config.rs
cargo build -p wordcartel > "$L/build.log" 2>&1; build_rc=$?
touch wordcartel/src/config.rs
cargo clippy --workspace --all-targets > "$L/clippy.log" 2>&1; clippy_rc=$?
cargo test --no-run > "$L/norun.log" 2>&1; norun_rc=$?
```

Result: `build=0 clippy=0 norun=0`. `grep -n "warning:" "$L/build.log" "$L/clippy.log"`
→ no matches (`no warnings found`).

PTY smoke suite:

```zsh
scripts/smoke/run.sh > "$L/smoke.log" 2>&1; smoke_rc=$?
```

`smoke_rc=0`. One-line summary, quoted verbatim:

```
smoke: 9/9 PASS
```

(All nine steps s1–s9 individually reported PASS; s9 is a step added since the S8
effort's last recorded "8/8" summary in the project's memory notes — not a
discrepancy, just suite growth.)

## Conclusion

Both `[files]` guard assertions (`files_type_filter_unknown_warns_and_defaults_documents`'s
warning check, and `files_filters_default_on_absent`'s default-value check) still bear
load after the H31 uniqueness fix folded the three scratch-path helpers into `load_cfg`:
each mutation, changing exactly one property, killed exactly the one assertion named for
it and no other. The working tree was confirmed byte-for-byte restored after each
mutation and again at the end. All merge gates (`cargo test`, `cargo build`,
`cargo clippy --workspace --all-targets`, `cargo test --no-run`) are clean on a forced
recompile; the PTY smoke suite (advisory) is green at `smoke: 9/9 PASS`.
