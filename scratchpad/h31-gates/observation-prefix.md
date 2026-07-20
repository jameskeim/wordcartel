# H31 Task 1 — pre-fix mechanism observation (D4)

Recorded before any path change (Task 2 has not run). This is the evidence Task 3's
attribution check compares against.

## Binary

```
/home/jkeim/projects/groundwords/target/debug/deps/wordcartel-801988c60b196269
```

## Harness runs

The step-6 30-run measurement (`scratchpad/h31-gates/run_n.sh 30 "$OUT" 1776`) came back
**0/30 failures — inconclusive per spec §7.1**, so it was followed by the spec-mandated
60-run re-run:

```
scratchpad/h31-gates/run_n.sh 60 "$OUT2" 1776
```

`SUMMARY:` line, verbatim:

```
SUMMARY: runs=60 failures=4 expected_total=1776 threads=32 binary=/home/jkeim/projects/groundwords/target/debug/deps/wordcartel-801988c60b196269
```

Harness exit code: `0` (trustworthy — not void).

Failure count: **4 out of 60** — runs 9, 12, 24, 49. All four are the same test:
`config::tests::files_type_filter_unknown_warns_and_defaults_documents`. No other test
failed in any of the 60 runs.

## Verbatim panic block (run 9)

```
thread 'config::tests::files_type_filter_unknown_warns_and_defaults_documents' (2042499) panicked at wordcartel/src/config.rs:1340:9:
the invalid-value arm must warn by name (H31 diagnostic); warns was: ["config: cannot read /tmp/wcartel-cfg-2042104-unknown.toml: No such file or directory (os error 2)"]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


failures:
    config::tests::files_type_filter_unknown_warns_and_defaults_documents

test result: FAILED. 1775 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out; finished in 4.27s
```

The failure lands at `config.rs:1340:9` — the `assert!` (the warning-presence check) added
in step 2, *not* the `assert_eq!` above it — identified by the custom message text "the
invalid-value arm must warn by name (H31 diagnostic)". The `warns` vector holds the
read-error string `config: cannot read /tmp/wcartel-cfg-<pid>-unknown.toml: No such file or
directory (os error 2)` in place of the expected `files.type_filter` warning: exactly the
diagnosed mechanism — `load_clip`'s `remove_file` deletes the shared-name scratch path
after `load_files` (or vice versa, ordering nondeterministic under 32 threads) has written
it but before `load_files`'s own `File::open` runs.

Runs 9, 12, and 49 all show this identical `cannot read … No such file or directory`
signature. Run 24 shows a related but distinct symptom from the same shared-path race — a
*torn write* rather than a delete-before-read:

```
thread 'config::tests::files_type_filter_unknown_warns_and_defaults_documents' (2072307) panicked at wordcartel/src/config.rs:1340:9:
the invalid-value arm must warn by name (H31 diagnostic); warns was: ["config: parse error in /tmp/wcartel-cfg-2071912-unknown.toml: TOML parse error at line 2, column 25\n  |\n2 | type_filter = \"spreadshe\n  |                         ^\ninvalid basic string\n"]
```

Here the other test's `File::write` overlapped in-flight with this test's `File::open` +
read on the same shared `…-unknown.toml` path, so the reader observed a partially-written
file (truncated at `"spreadshe`) rather than a missing one. Both variants share the same
root cause named in `DISPATCH-CONSTRAINTS.md`: `load_files`/`load_clip`/`load_diag` collide
on a path keyed only by `process::id()` (constant per binary run) + caller-supplied `name`,
and two callers pass `"unknown"`.

## Pass-condition check (spec §7.1)

- [x] at least one of the measured runs failed (4/60)
- [x] the failing test is `files_type_filter_unknown_warns_and_defaults_documents`
- [x] it failed at the warning assertion (H31 diagnostic message, line 1340), not the
      `assert_eq!` above it
- [x] the printed `warns` contains the read-error string `config: cannot read` (runs 9, 12,
      49; run 24 is the torn-write variant of the same race, noted above for completeness)
