#!/bin/zsh
# H31 verification harness — the ONE audited copy. Tasks 1, 3 and 4 all invoke this file.
#
#   usage: run_n.sh <N> <outdir> <expected_total>
#
#   <N>              number of whole-binary runs (positive integer)
#   <outdir>         directory for run-<i>.log and list.txt (created if absent, must be writable)
#   <expected_total> tests that must be ACCOUNTED FOR on every run, i.e. passed + failed.
#                    DERIVED per task, never a magic constant: baseline 1776 on main@60be3d1
#                    (1777 #[test] attributes under wordcartel/src minus the one #[ignore]d
#                    r1_typing_latency_bench in e2e.rs) + #[test]s this branch adds - removes.
#
# Exit 2 = integrity violation. The measurement is VOID — do not interpret any number it printed.
# Exit 0 = the runs happened and are trustworthy. Test FAILURES are reported, not treated as
#          harness errors: Task 1 legitimately expects failures (it observes the flake pre-fix).
#
# Every check below exists because its absence would let this script report success while the
# property it names is false. Do not relax one without replacing it with something stronger.
set -u

fatal() { print -r -- "FATAL: $*"; exit 2 }

# ---------------------------------------------------------------- arguments
[[ $# -eq 3 ]] || fatal "usage: run_n.sh <N> <outdir> <expected_total>"
N=$1; OUT=$2; EXPECTED=$3

# Validate VALUES, not just the argument count. `seq 1 0` expands to nothing, so an N of 0 or a
# non-numeric N would run zero tests and still print a success summary — a harness reporting
# success without running anything is the purest form of the defect this effort exists to remove.
[[ "$N"        == <-> && $N        -ge 1 ]] || fatal "N must be a positive integer, got '$N'"
[[ "$EXPECTED" == <-> && $EXPECTED -ge 1 ]] || fatal "expected_total must be a positive integer, got '$EXPECTED'"
mkdir -p "$OUT" || fatal "cannot create outdir '$OUT'"
[[ -d "$OUT" && -w "$OUT" ]] || fatal "outdir '$OUT' is not a writable directory"

# ---------------------------------------------------------------- concurrency policy
# THE WHOLE EFFORT IS INVISIBLE AT --test-threads=1. A run with defeated concurrency would
# produce a clean summary from a run that never exercised the property under validation, so
# the thread count is an integrity check, and it is RECORDED in the summary as evidence
# rather than assumed.
[[ -z "${RUST_TEST_SHUFFLE:-}" ]] || fatal "RUST_TEST_SHUFFLE is set — ordering assumptions void"
CORES=$(nproc)
if [[ -n "${RUST_TEST_THREADS:-}" ]]; then
  fatal "RUST_TEST_THREADS='$RUST_TEST_THREADS' is set — unset it; libtest must use full parallelism ($CORES)"
fi
THREADS=$CORES   # libtest defaults to available parallelism when RUST_TEST_THREADS is unset
FLOOR=16
[[ $THREADS -ge $FLOOR ]] || fatal "effective thread count $THREADS < floor $FLOOR — the 10/60 baseline was measured at 32; a lower count is not comparable"

# ---------------------------------------------------------------- binary selection
# From cargo's JSON artifact stream. NEVER an `ls -t` glob.
BIN=$(cargo test -p wordcartel --lib --no-run --message-format=json 2>/dev/null \
  | jq -r 'select(.reason=="compiler-artifact")
           | select(.target.kind[]=="lib")
           | select(.executable != null) | .executable' | tail -1)
[[ -n "$BIN" && -x "$BIN" ]] || fatal "no lib test binary from cargo's JSON artifact stream"

# ---------------------------------------------------------------- presence check
# A 0-failure result is MEANINGLESS if the tests are not in the binary: a botched fold that
# dropped or renamed the flaky test would otherwise score a perfect run. Match libtest's exact
# line format `<full::path>: test` — a substring grep would be satisfied by a RENAMED test that
# merely contains the original name, defeating the very check this is.
"$BIN" --list > "$OUT/list.txt" 2>&1; rc=$?
[[ $rc -eq 0 ]] || fatal "--list failed rc=$rc"
for t in config::tests::files_type_filter_unknown_warns_and_defaults_documents \
         config::tests::clipboard_provider_unknown_warns_and_defaults_auto; do
  grep -qx -- "$t: test" "$OUT/list.txt" || fatal "'$t: test' absent from binary (exact-line match)"
done

# ---------------------------------------------------------------- runs
print -r -- "binary:  $BIN"
print -r -- "threads: $THREADS (cores=$CORES, RUST_TEST_THREADS unset, no shuffle)"
print -r -- "expected_total: $EXPECTED   runs: $N"

fails=0
for i in $(seq 1 $N); do
  LOG="$OUT/run-$i.log"
  "$BIN" > "$LOG" 2>&1; rc=$?

  # PER-FILE integrity. An aggregate `grep | sort | uniq -c` across all logs would let a log
  # with zero result lines cancel one with two — it READS like a per-file guarantee and is not
  # one. This cannot cancel.
  nres=$(grep -c '^test result:' "$LOG")
  [[ $nres -eq 1 ]] || fatal "$LOG has $nres 'test result:' lines (want exactly 1)"

  line=$(grep '^test result:' "$LOG")
  passed=$(print -r -- "$line"   | awk '{for(i=1;i<=NF;i++) if($i=="passed;")  print $(i-1)}')
  failed=$(print -r -- "$line"   | awk '{for(i=1;i<=NF;i++) if($i=="failed;")  print $(i-1)}')
  filtered=$(print -r -- "$line" | awk '{for(i=1;i<=NF;i++) if($i=="filtered") print $(i-1)}')
  [[ -n "$passed" && -n "$failed" && -n "$filtered" ]] || fatal "$LOG: could not parse '$line'"
  [[ $filtered -eq 0 ]] || fatal "$LOG filtered=$filtered — a filtered run is not a whole-suite run"
  [[ $((passed + failed)) -eq $EXPECTED ]] \
    || fatal "$LOG passed+failed=$((passed + failed)), expected $EXPECTED"

  # Attribute failures by parsing the `failures:` BLOCK — never a bare test-name grep, because
  # libtest prints the test name for PASSING runs too.
  names=$(awk '/^failures:$/{blk=1; next} /^test result:/{blk=0} blk && /^    [a-zA-Z]/{print $1}' "$LOG")
  if [[ -n "$names" ]]; then
    fails=$((fails + 1))
    print -r -- "run $i FAILED: $names"
  fi
done

print -r -- "SUMMARY: runs=$N failures=$fails expected_total=$EXPECTED threads=$THREADS binary=$BIN"
