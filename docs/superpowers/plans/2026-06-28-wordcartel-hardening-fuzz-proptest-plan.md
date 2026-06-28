# Wordcartel Hardening Plan — Pre-Plugin (Effort P) Hardening

**Status:** Scope finalized (Codex-reviewed ×3 + a blind-spot deep-analysis pass).
**BUG-1 and BUG-2 already fixed and merged** (`main` @ `c26addf`).
**Date:** 2026-06-28

**Context:** ~8 efforts merged; the pure `wordcartel-core` is mature. Goal: drive out
panic-class and data-corruption bugs and lock invariants + public surfaces BEFORE
Effort P turns the registry/keymap/event/read surfaces into a public contract and lets
Lua drive the `apply` pipeline with machine-generated input.

**Two project invariants this protects (priority order):** (1) **No data loss / no
crash** — a panic in a terminal editor is terminal-hostage + lost work. (2)
**Responsiveness** — no super-linear blowup on pathological input.

## 0. The reframe (from the Codex blind-spot pass)

The real pre-P data-loss/crash surface is the **imperative shell** — IO durability,
worker-thread panics, resource ceilings, external-mod detection, and the edit boundary
— which needs **fault injection, panic isolation, and an adversarial boundary harness**
(techniques proptest/fuzz can't provide). Pure-core property/fuzz is valuable but is NOT
the long pole. This plan is therefore **shell-durability-first**, with a trimmed core
property/fuzz set, and a clearly separated "comprehensive / later" list.

**Two concrete bugs found by that pass are already DONE** (merged `c26addf`), and stand
as the template for the categories they belong to:
- ✅ **BUG-1** (crash / plugin blocker): job worker now `catch_unwind`s a panicking job
  (survives instead of silently killing the job system); panic hook gated to the main
  thread so a worker panic can't tear down the terminal. (`jobs.rs`, `term.rs`.)
- ✅ **BUG-2** (silent data loss): `FileFingerprint` gained a content hash → a same-size
  edit within one mtime tick is now caught by the external-mod check. (`save.rs`.)

---

## 1. D0 — the governing decision (Phase 0; everything keys off it)

The core has an **inconsistent invariant-violation policy**: `ChangeSet::insert/delete`
**silently clamp in release** (debug_assert only, change.rs:38,58); `TextBuffer`
insert/delete/slice **hard-assert/panic in release** (buffer.rs:30–63); `Selection`
and `ChangeSet` expose **public fields** (change.rs:10,17; selection.rs:13) so a caller
can build an inconsistent value that drives `apply`'s unchecked `pos += n` into a buffer
panic (change.rs:83–91) — `apply` doesn't even check `buf.len() == len_before`.

**Decision (D0) — valid-by-construction + Result at the edit boundary, NOT clamp for
edits.** Silently clamping a bad *edit* offset is **worse than crashing** (it becomes a
wrong edit = corruption). So:
- Make `ChangeSet` and `Selection` **valid-by-construction** (private fields + validated
  constructors); `apply` checks `buf.len() == len_before` and rejects invalid input
  *before* mutating.
- At the **shell/plugin → core edit boundary**, return `Result` (validate, never
  silently mutate on a bad offset). Per-command, not per-frame → no hot-path `Result`
  plumbing reaches render.
- Reserve **clamp/snap ONLY for explicit UI/nav recovery** (cursor restore, resume
  offsets — `nav::clamp_snap`, app.rs:351,371,411), never for silent edit mutation.

**Prerequisite migration (gates T5/T6):** private fields break real construction sites —
the shell builds raw `ChangeSet { ops, len_before, len_after }` at `commands.rs:125,157`
(`build_multi_replace`/`build_range_replace`) and core tests at `change.rs:430,437`.
First define the public validated-constructor API (keep `insert`/`delete` + add a
checked `from_ops`) and migrate these sites.

---

## 2. MINIMAL-VIABLE HARDENING (do before P; ranked by ROI)

Each workstream lists the detailed targets it contains (catalog in §3).

| # | Workstream | Contains | Priority / status |
|---|---|---|---|
| **M1** | **D0 + valid-by-construction + migration** | §1 decision; constructor migration; **T5** (Selection invariants), **T6** (ChangeSet validity) | MUST-before-P |
| **M2** | **Adversarial boundary harness** (highest-leverage, NEW) | Drive valid + invalid `Transaction`s, invalid ranges, synthetic/late job completions, close/switch-mid-job, quit-drain, **fake IO/clock/executor** through `reduce`/`apply_job_result`. Directly tests the seam P exposes to Lua. | MUST-before-P |
| **M3** | **IO fault injection** | save / save-as / swap / recovery-dump / session save+load: disk-full, partial-write, fsync/rename fail, leftover-temp. (`file.rs:197–231`, `swap.rs:197–211`, `state.rs:81–98`, `app.rs:2148` save-errs-ignored.) Durability can't be fuzzed → fault injection. | MUST-before-P |
| **M4** | **Async / panic hardening** | ✅ BUG-1 done (worker + hook). Remaining: `catch_unwind` for filter/transform/dispatch threads (`filter.rs:346`, `transform.rs:109`, `registry.rs:434`); a uniform panic→`Msg` surfacing (degrade, don't abort); dead-worker visible handling. | MUST-before-P (BUG-1 done) |
| **M5** | **Resource caps** | Central quotas: document-open size (`file.rs:60`), paste/filter/plugin output, undo history (unbounded `Vec<Revision>`, `history.rs:50`), search matches (`search_overlay.rs:25`), scratch/session size (`app.rs:2121`). **Gates fuzz CI** (else fuzzing = OOM/timeout). | MUST-before-P |
| **M6** | **External-mod hardening** | ✅ BUG-2 done (content hash). Optional follow: stat-first-then-hash optimization if save latency shows. | DONE |
| **M7** | **Minimal core property/fuzz** | **T1** (TextBuffer model oracle), **T2** (apply==splice + invert), **T3** (map_pos bounds/boundary/monotonic), **T4** (undo/redo round-trip); **F1** (apply pipeline — #1 data-loss fuzz target), **F2** (block_tree incremental ≡ full). | MUST-before-P (this subset only) |

---

## 3. Target catalog (detailed invariants)

**Existing coverage to EXTEND not duplicate:** `change.rs:320,387` (invert round-trip),
`selection.rs:115` (mapped-pos bounds), `tests/integration.rs:31` (undo round-trip), the
`block_tree`/`layout` suites. Each target is framed as gap-filling.

### Property tests (Track A — `proptest`, already a dev-dep, in-suite/CI)
| # | Target | Invariant (gap over existing) | Scope |
|---|---|---|---|
| T1 | `TextBuffer` insert/delete/slice (buffer.rs:30–63) | NEW model oracle vs `Vec<char>` model; content+len equal each step; off-boundary offsets rejected/Err per D0, never UB. Corpus: ASCII+é/中/🙂/ZWJ/combining. | M7 |
| T2 | `ChangeSet::apply`/`invert` (change.rs:83,99) | ADD apply==naive splice, multi-op, full unicode, `doc_len==0`, boundary edits. | M7 |
| T3 | `map_pos`/`map_pos_before` (change.rs:125–179) | ADD on-char-boundary + monotonicity + before/after bias. | M7 |
| T4 | `History` undo/redo (history.rs) | ADD redo exact, selection in-bounds+boundary, `version` strictly increases, coalescing loses nothing. | M7 |
| T5 | `Selection` invariants (selection.rs:50–56) | Public validated constructors guarantee `primary<len`, `from()≤to()`, `map()` preserves. Test public constructors only. | M1 |
| T6 | `ChangeSet` validity (change.rs:10,17,83–91) | Retain+Delete sum to `len_before`; Retain+Insert sum to `len_after`; op boundaries UTF-8; **`apply` checks `buf.len()==len_before` and rejects before mutating**. | M1 |

### Fuzz targets (Track B — `cargo-fuzz`/`libfuzzer-sys`/`arbitrary`, new `fuzz/` crate)
| # | Target | Harness / oracle | Scope |
|---|---|---|---|
| F0 | stand up `fuzz/` crate | `arbitrary` `EditOp` enum + unicode-biased string gen; seed corpus from proptest corpora | M7 (for F1/F2) |
| F1 | **apply pipeline** (char-boundary wall) | arbitrary doc + `Vec<EditOp>` via real ChangeSet/TextBuffer; never panics (per D0); matches `Vec<char>` model. #1 data-loss target. | M7 |
| F2 | `block_tree` incremental ≡ full | **lift the oracle into core behind `cfg(any(test,fuzzing))`** (the macro `assert_all_paths_agree!`, block_tree_oracle.rs:24,38, is test-local/`prop_assert_eq!` — a fuzz crate can't import it); then arbitrary doc+`Edit` → incremental ≡ full. | M7 |
| F3 | `ChangeSet` construction | arbitrary `(pos,del,ins)` via validated constructor; consistency; no panic. | LATER |
| F4 | `search` (search.rs) | arbitrary regex+doc; `compile` Ok/graceful; `all_matches` terminates (zero-width, search.rs:63); `expand_replacement` (search.rs:96) no OOB. | LATER |
| F5 | `layout` ColMap (layout.rs:751) | arbitrary doc+width; source↔visual round-trip; no panic on wide/ZWJ/tabs. | LATER |
| F6 | `outline` (outline.rs) | section/body/heading ranges ordered, in-bounds, char-boundary, non-overlapping. | LATER |

**Byte-slice boundary audit (folded into M2/B2):** core entry points that slice by
arbitrary byte range — `TextSource` slice (block_tree.rs:31), parser `text[range]`
(block_tree.rs:398), `apply_edit` (block_tree.rs:1100), `expand_replacement`
(search.rs:96) — each fuzz input must snap, OR the range is documented as
internally-derived-and-trusted (with a property pinning the derivation).

### Responsiveness / complexity (Track C)
| # | Target | Approach | Scope |
|---|---|---|---|
| C1 | hot-path complexity audit | `derive`/layout/search/count per keystroke bounded by visible+edited, not doc size. | M7-adjacent (audit) |
| C2a | incremental ≡ full | = F2/the oracle; always holds. | M7 |
| C2b | locality property | **needs instrumentation first** — `UpdateOutcome` exposes only `reason`+`reparsed_bytes` (block_tree.rs:475); extend to region bounds + widen flags. Property holds ONLY for *known-local* edits that avoid EVERY widen source: HTML/front-matter fallback (642–644, ~664), `WidenToEnd` (749–751), container pull-back (592), container/table merge (719), `needs_widen_to_end` ref-def/fence (900). A no-overlap edit is NOT a widen source (can resolve `Local`, 793). | LATER |
| C3 | pathological corpus | 5MB single line, 100k tiny blocks, nested lists, all-combining-marks, giant table/fence; doubles as fuzz seed (F1/F2/F5). | LATER (corpus) |

---

## 4. Tooling & CI

- **Track A:** in `cargo test` (CI today). `PROPTEST_CASES` split — default (~256) per PR,
  elevated nightly (~4096) for invariant tests. Shrink-failures → named regression cases
  (precedent: block_tree_oracle.rs "CE1/CE2/C3" cases).
- **Track B:** new top-level `fuzz/` crate (own Cargo.toml, libfuzzer target, checked-in
  `corpus/`). Minimal before P = local/manual campaign on F1/F2; full standing CI
  (~60 s/PR + nightly) is "comprehensive/later". Crash → minimized repro → pinned as a
  normal-suite regression test. `#![forbid(unsafe_code)]` binds the core only (not deps
  like ropey/smartstring — a dep-level UB would need ASan; out of scope).
- **M2/M3 harness:** fake `Executor`/`Clock`/filesystem (the codebase already has a test
  `Executor` + `TestClock`); schedule-permutation + IO-failure injection — proptest is
  single-threaded and will NOT find late-result/worker-panic/stale-overlay interleavings.

## 5. Sequencing (ordering hazards)

1. **D0 (M1) before T1–T6 / F1 / F3** — else the harness encodes unstable policy.
2. **Transaction validation (M1/M2) before the plugin API is specced** — don't spec Lua
   `apply(Transaction)` over today's public raw fields.
3. **Resource caps (M5) before fuzz CI** — else fuzzing is an OOM/timeout exercise.
4. **Job-panic isolation (M4; BUG-1 ✅) before any M-async confidence** and all plugin async.
5. **IO fault injection (M3) before the "no data loss" done-criteria.**
6. C2b locality follows F2 correctness; it does NOT block the minimal path.

## 6. Definition of done (the bar before Effort P)

- D0 decided ✅ and **applied** (M1): `ChangeSet`/`Selection` valid-by-construction;
  edit boundary returns `Result`; no layer silently corrupts or panics-in-release on a
  bad offset. T5/T6 green.
- **M2 boundary harness** exercises valid+invalid Transactions, late/stale job results,
  close/switch-mid-job, quit-drain, and injected IO failures — all degrade, none crash.
- **M3 IO fault injection** green for save/save-as/swap/recovery/session.
- **M4**: filter/transform/dispatch panics caught + surfaced; no terminal restore from a
  worker thread (BUG-1 ✅).
- **M5 resource caps** in place (open size, output, undo, matches, scratch/session).
- **M6** external-mod fingerprint ✅ (BUG-2).
- **M7**: T1–T4 + T6 green at elevated cases; F1/F2 run a manual campaign with zero new
  crashes (crashes → fixed + pinned).
- No known **data-loss** or **responsiveness** bug open; the §18.3 public surfaces
  reviewed and considered settled.

## 7. Comprehensive / later (NOT required to de-risk P)

1. Full standing cargo-fuzz CI for F3–F6.
2. C2b locality instrumentation + pathological-latency gates (C3 corpus).
3. Plugin-API fuzzing (during/after P).
4. Differential markdown-parser testing beyond the full-parse oracle.
5. Filter/subprocess hostility tests (huge stderr, cap, ignore-terminate, write-after-cancel, shell quoting — filter.rs:136,173,204).
6. Encoding policy (BOM / CRLF preservation / normalization / bidi — file.rs:48, commands.rs:249 always-LF). Invalid UTF-8 itself is already handled.
7. Terminal/signal integration (resize storm, SIGTERM, raw-mode leaks).
8. General performance benchmarks/profiling (separate perf effort; reuses the F1/F2 generators + C3 corpus).

## 8. Out of scope (this campaign)

- Fuzzing the imperative shell end-to-end (terminal/IO/threads) — the M2 harness +
  existing "3× parallel stable" checks fit it. (Pure shell helpers — keymap chord
  parser, `expand_path`, filter-spec parsing, toml round-trips — are cheap to proptest
  opportunistically.)
- New features. Hardening only.
