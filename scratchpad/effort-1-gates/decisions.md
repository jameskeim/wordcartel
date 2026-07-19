# Effort ① — decisions ratified by the human

Brainstorm 2026-07-19. Each entry records the choice AND the reasoning, so a later reader can tell
whether a new fact should reopen it.

---

## D1 — Effort scope: test-isolation as a class, plus the EPIPE bug

**Chosen: C** (of A tight-two-bugs / B all-five-as-mapped / C isolation-class + EPIPE).

① is **not** the five backlog items as filed. It is:
1. the test-isolation class (see D3), and
2. H20's `run_subprocess` EPIPE bug, which the grounding showed is a **production** bug, not a flake.

**Deferred out, each for a mapped reason — do NOT quietly re-absorb them:**
- **H26** (fs-chokepoint use-tree parsing) — the filed solution needs a NEW dependency (`syn`);
  `wordcartel`'s dev-deps are `proptest` + `tempfile` only, and `syn`/`proc-macro2` sit in
  `Cargo.lock` solely as transitive deps of `bindgen`/`burn-derive`, unreachable from any workspace
  member. Collides with the standing dependency-weight concern (H2). Needs its own scoping.
- **H27** (DispatchCtx collapse) — measured blast radius is **10 production call sites vs ~150 test
  call sites** (essentially all on `app::reduce`). This is the "surprise on size" condition the
  post-C5 map recorded as the trigger to drop it back to opportunistic. Also: three of the seven
  functions build a SECOND, differently-shaped bundle (`registry::Ctx`, which owns `&mut Editor` and
  holds owned `msg_tx`/`fs` because `dispatch_filter` spawns a `'static` thread) — collapsing to
  `DispatchCtx` would not remove that construction.
- **H28** (un-pumped picker tests) — smaller and different than filed. 18/20 destination-Enter tests
  already pump; all 20 drive the real intercept. The two named tests' doc comments record that
  pumping was tried live and REVERTED, and the map confirms why: once a listing lands, `rederive`
  puts `".."` at `entries[0]`, `selected` stays 0, and Row 1's guard becomes true purely off
  `trimmed.is_empty()` → `Descend`, so the empty-path warning is genuinely unreachable. The real
  question is therefore a BEHAVIOUR one — is that warning reachable in production at all? — not a
  test-hygiene one. Do not "fix" it by making the tests pump; that just deletes the assertion.

---

## D2 — H20 is a production bug, and this effort fixes it

`filter::run_subprocess`'s poll loop treats ANY non-`TimedOut` io error — including `BrokenPipe` —
as an unexpected failure: it kills the child and returns `FilterError::Spawn`, **discarding output
it had already captured**.

```rust
} else {
    // Unexpected I/O error.
    let _ = child.terminate();
    let _ = child.kill();
    return Err(FilterError::Spawn(ce.error.to_string()));
}
```

So any `!` filter whose command exits before consuming all of stdin — `head -1`, `grep -q`,
`sed 3q` — races us, and when the child wins we kill it and throw its output away.

**Measured:** 18/18 captured failures identical, verbatim
`expected NonZero, got Err(Spawn("Broken pipe (os error 32)"))` at `filter.rs:490`. Rate tracks
process contention: 0/300 isolated → 4/60 at default threads → **14/36 (~39%) under six-way
parallel load**.

**The July 2026-07-13 triage was wrong on mechanism** — it guessed `child.wait()` returning
`Undetermined`. That path yields `NonZero`, not `Spawn`, and was never the failure. It also
recorded "EPIPE-on-stdin handled by the subprocess crate"; it is not.

Correct behaviour is standard Unix filter semantics: an early-exiting child is normal, so stop
writing stdin, finish draining stdout/stderr, then wait and report the child's real status/output.
Exact drain-then-wait mechanics are a spec/plan question.

---

## D3 — Isolation mechanism: proportional, case by case

**Chosen: D** (of A seams-everywhere / B serialize-all / C order-independence-all / D proportional).

The cases genuinely differ:
- **Real-resource cases** (`session.toml`, `state_dir()`) are a **damage** problem — only a seam
  fixes them. C5 already established the pattern (`open_recent_in`, `state::load_in`/`save_in`), so
  this is applying an existing convention, not inventing a second one.
- **Pure in-process globals** are a **visibility** problem — order-independence where the assertion
  allows it, a lock where it does not. `LAST_GOOD` is the latter: the test asserts the global's
  value, so order-independence cannot work there.

Rejected uniform mechanisms because they either over-engineer the cheap cases or under-fix the
dangerous one.

---

## D4 — Class membership: damage class + demonstrated flake

**Chosen: B** (of A demonstrated-only / B damage+demonstrated / C whole-class sweep).

**IN:**
- `recovery::LAST_GOOD` — measured flake (3/60 default threads, 0/300 isolated, 0/15 at t=1).
  Mechanism: one process-global `Mutex` that `apply`/`undo`/`redo` write from nearly every editor
  test; this test reads it with no isolation, so a concurrent unrelated test's write lands between
  its write and read. Panics captured, e.g. `left: Some("hello\nabc")` / `right: Some("Xabc\n")`.
- `persist_session_for_test` ×3 (`session_restore.rs`) — writes the REAL
  `~/.local/state/wordcartel/session.toml` with **no restore at all**. C5 fixed exactly this class
  for `recents::open_recent` and left the sibling exposed.
- every test writing the real `state_dir()` (`swap.rs`, `recovery.rs`) — partial cleanup only, by
  explicit in-repo comment.
- the two fixed `/tmp/wordcartel_*_<pid>` paths (`search_ui.rs`, `diagnostics_run.rs`).

**OUT (no observed failure — revisit only if one appears):** `plugin::INTERN_POOL` (an in-repo
comment notes drift, but nothing has failed), `file_browser::LISTING_EPOCH`,
`cursor_style::restore::EVER_WROTE` (its tests were already written order-independent).

The rule being enforced — **"a test never touches the user's real files"** — is checkable rather
than judgment-based, which is why it beats "fix what looks risky."

---

## D5 — Durability: make it structurally impossible, not detected

**Chosen: B** (of A scanner-guard / B structural-enforcement / C both / D no guard).

In test builds, `state_dir()` and its siblings must not hand back the real path unless a test
explicitly overrides it — a test that forgets gets a temp dir or a loud failure, never the user's
session file. No allow-list, no scanner, nothing to evade.

**Why not another textual scanner**, given the tree already has four guard tests: the H26 map
measured what that approach actually buys — **5 of 6 evasion routes uncaught**, the scanner's own
7 self-checks covering **none** of them, one gap disclosed in its own source, and **37 of 51
markers** being blanket `(w)` wrapper exemptions whose prose is never semantically checked.
Answering a trust-in-gates problem by adding a second textual scanner would be self-defeating.

**Known risk, to settle during the spec:** B changes production code for a test-only concern, and
any test that legitimately needs the real directory needs a deliberate escape hatch — which is a
small allow-list wearing a different hat. Count those tests during specification; the map suggests
few or none.

---

## Standing context for whoever specs this

- **There is no CI.** No `.github/workflows`, no hooks, no gate script. Every stated merge gate runs
  only because a human or agent follows CLAUDE.md. Relevant to any claim that something "is
  enforced."
- **Four guard tests exist** and are the local idiom: `module_budgets` (hub line-count caps),
  `backlog` (manifest↔dashboard bijection + schema), `fs_chokepoint` (FS access routes through the
  seam), `edit_seam` (H22 edit chokepoint two-statement pattern).
- **Machine has 32 cores**; failures appear only at `--test-threads` ≥ 32, never at 1 or 4. Any fix
  must be validated at default threading, not in isolation.
- **A fifth false-invariant comment** was found in passing:
  `session_restore.rs::file_browser_enter_on_file_opens_it_when_clean` says it "simulates Enter" but
  calls `open_into_current` directly and never touches picker input. Same class as C5's four.
- Maps: `map-h26.md`, `map-h27.md`, `map-h28.md`, `map-flakes.md`, `map-testinfra.md` in this
  directory. All were produced by agents instructed to report facts only, no recommendations.
