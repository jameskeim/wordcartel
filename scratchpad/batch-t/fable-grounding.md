# Batch T — grounding verification and scoping (Fable, 2026-07-27, tree `6d3a213`)

Every claim below was checked against the real source at `6d3a213`. Where a claim rests on
inference rather than reading, it is labeled as such. No source file was modified; the only
external evidence used is the locally installed ltex-ls-plus's bundled LanguageTool 6.8 jars
(`/usr/share/java/ltex-ls-plus/`), i.e. the very binary `LtexEngine::spawn_command()` spawns.

---

## §1 Grounding corrections

Numbered. "VERIFIED" = brief/filing claim confirmed; "CORRECTED" = brief/filing wrong; "SETTLED" =
a §6 open question now decided on evidence.

### 1.1 SETTLED (§6.5): H38's spell-substring branch is LIVE — the fix is PIN, not delete
The question was whether a real ltex/LanguageTool diagnostic `code` can contain `spell` while
missing `MORFOLOGIK`/`HUNSPELL`/`SPELLER` (uppercased) — i.e. whether
`classify_spell_heuristic`'s code branch (`lsp_client.rs:69`) is reachable past
`LtexEngine::classify`'s short-circuit (`ltex_ls.rs:84-88`).

**It is, abundantly.** Extracting every `[A-Z_]*SPELL[A-Z_]*` identifier from the LanguageTool
6.8 jars bundled with the locally installed ltex-ls-plus yields hundreds of real rule ids that
contain `SPELL` but none of the three short-circuit substrings. Decisive examples:

- **`FR_SPELLING_RULE`** — verified to live in
  `org/languagetool/rules/fr/MorfologikFrenchSpellerRule.class` (language-fr-6.8.jar). This is
  the actual French *speller*: a French document's spelling errors carry this code.
- **`EN_CONTRACTION_SPELLING`**, **`OXFORD_SPELLING_*`**, `BRE_STYLE_OXFORD_SPELLING` — English
  rules, so the branch is reachable even in plain `en-US` usage, not only exotic languages.
- `OLD_SPELLING_RULE`, `DE/ES/CA/FR/PT_MULTITOKEN_SPELLING*`, `CONTRACTION_SPELLING_RULE`, …

Deleting the branch would misclassify these as `Grammar` (their `message` need not contain
"spell"). So H38's filed fix shape ("a fixture … asserted Spelling, kill condition: deleting the
branch must redden it") is the right direction, and the §6.5 alternative ("delete the branch")
is refuted.

*Inference boundary:* that ltex-ls-plus surfaces the LanguageTool rule id as the LSP diagnostic
`code` string is inferred from (a) the E10 spec's own citation of `MORFOLOGIK_RULE_*` as codes,
(b) `LtexEngine::classify` already matching codes-as-strings, and (c) the T11 live-probe
history. Not re-probed live here (a live probe would need a JVM warm + a French doc). Confidence
high; if the human wants it airtight, one live probe with `language: fr` settles it.

### 1.2 SETTLED (§6.4): H28's "empty path" warning IS production-reachable — by BOTH routes
This **inverts the filed fix** ("retire the tests and the dead branch") and **refutes the H28
prose's** central claim ("the empty-path warning is genuinely unreachable once a listing lands").

**Route A — the pre-listing async window. HOLDS, end-to-end:**
- Both production entries seed an EMPTY field: `open_save_as` (`prompts.rs:83-93`,
  `String::new()`) and `block_write` (`blocks_marked.rs:128-143`, `String::new()`).
- `Editor::open_destination_picker` (`editor.rs:1042-1064`) sets `entries: Vec::new()` and
  spawns the listing on a detached thread (`start_listing`, `file_browser.rs:423-438`); its own
  comment: "There is no synchronous listing path."
- The Enter arm of `file_browser_intercept::intercept` (`file_browser_intercept.rs:59-67`) calls
  `commit_destination` with NO guard on `awaiting_epoch`/`pending_dir` — nothing stops an Enter
  before `Msg::ListingDone` is folded.
- `commit_destination_with_probe` (`file_browser_commit.rs:350`) reads
  `fb.entries.get(fb.selected)` → `None` → Row 2 `_ => CommitOutcome::Nothing` → the Sticky
  Warning (`file_browser_commit.rs:363-371`).

A writer pressing ^KS then Enter before the listing lands (slow disk, NFS, cold cache) is in
EXACTLY the state the two tests construct. The tests are not asserting an unreachable state;
they are asserting the pre-listing window — which is a real production state by design.

**Route B — a highlighted Other/Unknown entry with a fully LANDED listing. HOLDS:**
- Production listings contain such rows: `fsx.rs` `kind_of` returns `Other` for
  fifo/socket/device (`fsx.rs:274-276`), `classify_entry` returns `Unknown` for stat failure
  (`fsx.rs:242`) and broken symlinks (`fsx.rs:268`, built into entries at
  `file_browser_listing.rs:232-234`).
- They survive into destination-mode `entries`: `filter_and_rank` (`file_browser_listing.rs:127-144`)
  exempts broken entries from the type filter unconditionally (`e.broken` at `:135`), and an
  extensionless fifo passes `is_document` (`:105`, "extensionless files are plausibly prose").
- A writer who ARROWS onto such a row (`highlight_navigated` true is irrelevant — Row 1 requires
  `Dir`, Row 2 requires `File`) with an empty field hits `_ => CommitOutcome::Nothing` — with a
  landed listing. The H28 prose's mechanism analysis only considered the untouched default
  highlight (`".."` = Dir at `entries[0]`); it missed the navigated non-Dir/non-File case.

**Consequence:** the tests stay, the branch stays. "Retire tests + dead branch" would have
deleted a production-reachable warning — and more: the `Nothing` arm also ABORTS an in-progress
quit drain (`file_browser_commit.rs:373-377`, the Effort-6 Codex-C2 rule), so "removing the dead
branch" would have silently touched quit semantics. (The brief did not mention the drain-abort.)

**The Route-B tail observation is confirmed as real:** in Route B the writer is told
"save-as: empty path" but the field's emptiness is not why the commit refused — the highlighted
entry is not a writable regular file. Select mode has per-kind refusal wording
(`classify_enter`, `file_browser.rs:207-223`); the destination path has none. That is a
legitimate (small) UX defect — but it is a BEHAVIORAL change and must be filed out of Batch T,
not fixed inside it (see §3).

### 1.3 CORRECTED (§6.1): the bare-`temp_dir()` population is 12 code sites, not 17
Total hits: **116 across 28 files** (brief's count verified; filing's "~105 across ~30" and its
per-file figures are stale — measured: `prompts.rs` 20 (filed ~14), `file_browser.rs` 17,
`app.rs` 11 (filed ~15), `jobs_apply.rs` 9, `render_overlays.rs` 7, `swap.rs` 6 (filed ~10)).

The brief's "17 bare" over-counts. Line-by-line classification of the 116:
- **98** `.join(format!(…))` constructions — 96 same-line + 2 line-wrapped
  (`prompts.rs:1263-1264`, `:1383-1384`), which the brief mis-bucketed as bare.
- **12** bare code sites: `prompts.rs:502,522,714,794,818` · `file_browser.rs:732,1086,1101,1120`
  · `save.rs:1265` · `chrome_geom.rs:882` · `swap.rs:682`.
- **3** comment lines (`prompts.rs:487,717`, `swap.rs:675`) — not code at all.
- **1** the seam's own impl (`test_support.rs`).
- **2** integration-test sites (separate crate, correctly filed out of scope).

And within the 12, `swap.rs:682` is `assert!(d.starts_with(std::env::temp_dir()))` — a semantic
assertion about the REAL system temp dir that can never be swept onto the seam.

### 1.4 CORRECTED in the strong form (§6.2): the bare group has LATENT exposure, not active decay
Claim examined: the ~17 bare sites "read a directory whose contents other processes control" —
i.e. the filing's in/out-of-scope call is backwards. Reality, site by site:
- Only **3** sites ever load a listing of the real shared temp dir:
  `prompts.rs:714,794,818` (they pump with `RealFs`; `test_fs()` = `RealFs`,
  `test_support.rs:31-33`). All three are defended BY CONSTRUCTION: the field is a non-empty
  ABSOLUTE path, so `highlight_is_navigated()` gates Row 1 off "regardless of whatever
  `temp_dir()` happens to sort first" (the `prompts.rs:715-718` comment), and no assertion reads
  `entries`. Content-independent today.
- `prompts.rs:502,522` (the H28 pair) never pump — shared-dir contents never load.
- `save.rs:1265` (Esc test), `file_browser.rs:732` (XOR test), `file_browser.rs:1086,1101`
  (footer literals, empty `listing`), `file_browser.rs:1120` (no-picker discard),
  `chrome_geom.rs:882` (geometry, hand-built entries) — the dir value is inert.
So: **no bare-site assertion depends on shared-dir contents at `6d3a213`.** The decay is latent
(a future edit could start depending on the listing), not active. The batch's sequencing
rationale ("nothing here can change shipped behavior; the sweep is safe volume") SURVIVES —
but only because the bare group must be left out of the sweep (or handled as a deliberate,
separately-reviewed semantic migration — fork F5).

### 1.5 VERIFIED (§6.2's other half): the join population genuinely has no decay
All 98 `.join` sites use `format!` with `std::process::id()` on the same line (grep: zero
exceptions), and the label strings are pairwise distinct (zero duplicate labels across the
workspace). "Individually correct today — pid-unique, one label per test — not the H31 collision
class" is TRUE as filed.

### 1.6 VERIFIED (§6.3): H28 and H36 collide on `prompts.rs:502` and `:522`
Both lines are H28's tests and members of H36's bare population. Any H36 execution must
sequence after (or around) H28's disposition. Confirmed as a real coordination constraint.

### 1.7 NEW — `swap.rs:48` is a `#[cfg(test)]` branch inside PRODUCTION `state_dir()`
Not in any test module: it is the Effort-① D5 test-build redirect chokepoint inside
`swap::state_dir()` (`swap.rs:46-49`). Its contract is a **stable per-process** directory —
every call must return the same path. `test_support::scratch_name` increments `SCRATCH_SEQ` per
call, so a naive sweep of this site would hand every `state_dir()` call a DIFFERENT directory
and break swap/session/recovery tests structurally. Must be excluded from H36 (or given a
`OnceLock`, which is a design change nobody asked for).

### 1.8 VERIFIED: H38's coverage hole is real, by reading the full test-reachability graph
`classify_spell_heuristic` has exactly one production caller (`ltex_ls.rs:90`); the only tests
that reach it are `ltex_ls.rs:151` `classify_maps_languagetool_speller_rules_to_spelling`, whose
fixtures either short-circuit in `LtexEngine::classify` (`MORFOLOGIK_RULE_EN_US`,
`GERMAN_SPELLER_RULE`) or exercise the MESSAGE path (`PASSIVE_VOICE` → Grammar; message-only →
Spelling). Nothing exercises the shared fn's CODE branch. Deleting `lsp_client.rs:69` reddens no
test. Both `TestEngine` impls define `classify(_d) → Grammar` and never reach it
(`lsp_client.rs:1163,1789`). The historical claim is also verified from git: the deleted
`vale_ls.rs` (`bed74a6^`) short-circuited on `code.contains("Spelling")` at its line 65, so
`classify_spelling_checks_by_name_else_heuristic` never reached the shared branch.

### 1.9 VERIFIED: harper's duplicate is pinned; the "intentionally identical" claim is untested
`harper_ls.rs:158` `classify_lsp` is byte-identical in body to `classify_spell_heuristic`, is
pinned by `classify_lsp_spelling_vs_grammar` (`harper_ls.rs:529-533`, including the code branch
via `"SpellCheck"`), and no test asserts the two bodies agree. Both facts as the brief stated.

### 1.10 Minor brief citation errors
- §3.2: "`file_browser.rs:256` returns `None`; the caller sets the Sticky Warning" — line 256 is
  `footer_target`'s `Nothing` arm (footer suppression). The warning is set in
  `commit_destination_with_probe`'s `Nothing` arm, `file_browser_commit.rs:363-371`.
- §3.3: "recents.rs:142,145" — the `Unknown` re-marking is in `apply_recents_probed` at
  `recents.rs:142-146` (loop bodies); substance correct.
- §2 code excerpts, `LtexEngine::classify`, the harper pin test, `EntryKind` production sites
  (`fsx.rs:242,268,274-276`), `file_browser.rs:170-180` glyphs, `:207-223` refusals: all
  verified accurate.

### 1.11 NEW — the `None`-highlight `Nothing` case is already unit-pinned; the kind-based one is not
`an_empty_field_with_no_highlight_commits_nothing` (`file_browser_commit.rs:2276-2283`) pins
`highlighted=None → Nothing`. NO test anywhere constructs an `EntryKind::Other` or
`EntryKind::Unknown` entry against `classify_destination_enter` — Route B's fall-through
(`_ => Nothing` on a non-Dir/non-File highlight) is unpinned. That `_` arm would silently absorb
a future fifth `EntryKind` variant too (the exact catch-all class the house style warns about).

---

## §2 Per-item resolution options with real blast radius

### H38 — pin `classify_spell_heuristic`'s code branch
The hole is real (1.8); the branch is live (1.1). "Delete the branch" is off the table.
"Do nothing" is not supported by the grounding — the branch classifies real spelling
diagnostics and nothing red-flags its deletion.

- **Option A — direct unit pin.** A test in `lsp_client.rs`'s test mod calling
  `classify_spell_heuristic` with a code containing `spell` and a message WITHOUT it (so the
  message path cannot rescue), asserted `Spelling`. Kill condition: deleting `lsp_client.rs:69`
  reddens it. Blast: 1 file, +1 test, ~10 lines.
- **Option B — engine-level pin with real ids.** Extend
  `classify_maps_languagetool_speller_rules_to_spelling` (`ltex_ls.rs:151`) with
  `FR_SPELLING_RULE` and/or `EN_CONTRACTION_SPELLING` asserted `Spelling` (messages without
  "spell"). Same kill condition, and it documents the live-input grounding (real LT 6.8 ids that
  miss the short-circuit). Blast: 1 file, +2 assertions.
- **Option C — A + B + an agreement matrix.** Also add a test driving the SAME fixture matrix
  through `classify_spell_heuristic` and harper's private `classify_lsp`, asserting equal
  outputs — closes the "two bodies intentionally identical, no test asserts it" gap (1.9).
  Needs the matrix to live where both fns are visible: `classify_lsp` is private to
  `harper_ls.rs`, so the agreement test goes in `harper_ls.rs`'s test mod (it can `use
  crate::lsp_client::classify_spell_heuristic` — same crate, `pub(crate)`). Blast: 2 files,
  +2-3 tests, ~30 lines. What could regress: nothing (test-only).

### H28 — the two un-pumped picker tests
The filed fix ("retire tests + dead branch") is refuted (1.2). The tests pin a real production
state and already redden if the `Nothing` arm or its message changes. Live options:

- **Option A — keep + re-document + pin Route B + file the tail.**
  1. Rewrite the two doc comments (`prompts.rs:482-494`, `:509-511`): the current text frames
     the state as a design oddity; the grounding shows it is the production pre-listing window.
     Also correct the H28 prose's "genuinely unreachable once a listing lands" when archiving.
  2. Add a Route-B pin: a unit test on `classify_destination_enter` with an
     `EntryKind::Unknown { broken: true }` (and/or `Other`) highlighted entry + empty field →
     `Nothing` (fills 1.11's gap; catch-all-absorption guard), and optionally one
     intercept-driven test (pumped listing over a scratch dir containing a broken symlink,
     navigate, Enter, assert the same Sticky Warning) — the latter proves Route B through the
     real seam, unix-gated.
  3. `scripts/backlog add` a NEW item for the behavioral tail: destination-mode refusals have no
     per-kind wording (Route B says "empty path" for a reason that isn't the field), mirroring
     select-mode `classify_enter`'s messages. Behavioral — outside Batch T.
  Blast: `prompts.rs` (comments only), `file_browser_commit.rs` +1 test, optionally
  `file_browser.rs`/e2e +1 test, backlog.toml + ux-backlog.md (new filing), H28 itself resolved.
  What could regress: nothing shipped; the unix-gated test adds a symlink fixture.
- **Option B — keep + re-document only.** Cheapest; leaves Route B and the `_` catch-all
  unpinned. Defensible but wastes the grounding.
- **Option C — the filed fix (retire).** REFUTED — would delete a production-reachable warning
  and disturb the quit-drain abort (1.2). Not viable.

### H36 — the sweep
The filed premise ("individually correct, no decay") is VERIFIED for the join population (1.5).
The value is uniformity plus deleting boilerplate: ~31 remove-then-create/cleanup lines and ~43
`create_dir_all` lines near join sites collapse into `scratch_dir()`. The cost is mechanical
churn across ~26 files of test code plus review/blame noise.

- **Option A — full sweep of the 98 join sites**, excluding: the 12 bare sites, `swap.rs:48`
  (1.7 — structural breakage if touched), the seam impl, the 2 integration-test sites.
  Mechanics: `temp_dir().join(format!("wc-x-{}.md", pid))` → `scratch_path("x.md")`;
  dir sites with `create_dir_all`/remove-dances → `scratch_dir("x")`. Labels keep their
  extensions (the seam appends the label last, so suffixes survive). Hazards to respect:
  tests constructing MULTIPLE related paths keep one call per path; tests asserting on
  file-NAME shapes (grep before sweeping each file); `swap.rs:682`'s `starts_with(temp_dir())`
  still passes (scratch paths live under temp_dir). Blast: ~26 files, ~98 sites, all
  `#[cfg(test)]`; zero shipped-behavior surface. What could regress: individual tests whose
  fixtures encode the old path shape — caught by `cargo test` immediately; the real cost is
  review volume.
- **Option B — close as not-worth-it.** The grounding honestly supports this: the population is
  healthy, the seam already serves new code, and the sweep buys uniformity + ~74 lines of
  deleted boilerplate at the price of ~26 files of blame churn. If chosen, record the closure
  rationale in the archive so the decision is on the record (the filing itself anticipated
  this: "review blast radius dwarfs the value on a no-decay item").
- **Option C — targeted sweep.** Only the files where the seam deletes real boilerplate (the
  dance/`create_dir_all` clusters — top 6 files ≈ 74 sites), leaving one-line path
  constructions alone. Middle cost, but leaves the "exactly one answer everywhere" goal unmet —
  arguably the worst of both (two idioms persist AND churn happens).

Separately decidable (fork F5): migrating the 3 pumped picker-seed bare sites
(`prompts.rs:714,794,818`) from the shared temp dir to a `scratch_dir()` — a deliberate,
small SEMANTIC hardening (deterministic listings; the `..` row still exists since a scratch dir
is never filesystem root), not part of the mechanical sweep.

---

## §3 One effort or not

**Keep Batch T as ONE effort — with two conditions.** The human's keep-whole preference holds
here on the merits, not just preference:

- All three items are test-integrity work on the same instrument; one spec/review context avoids
  re-explaining the picker decision table and the seam three times.
- H28 and H36 physically collide (`prompts.rs:502,522` — 1.6); resolving them in one branch with
  explicit internal ordering (H38 → H28 → H36-sweep-last) eliminates a cross-effort conflict.
- The batch premise "nothing here can change shipped behavior" SURVIVES the grounding, but only
  because of the two conditions:
  1. **H28's behavioral tail is filed OUT** (new backlog item for destination-mode per-kind
     refusal wording), not fixed in-batch. Fixing it in-batch would change a user-visible
     status message and break the premise.
  2. **The H36 sweep excludes the 12 bare sites and `swap.rs:48`** (1.3, 1.7). Sweeping bare
     sites is a semantic change to tests, not delegation; F5's three-site migration, if chosen,
     should be its own commit flagged as semantic, not buried in the mechanical sweep.

Note the batch's sequencing-rationale sentence needs one correction when the spec is written:
H28 is no longer "make the tests honest or retire them" — the grounding shows the tests were
honest all along and the FILING's fix direction was the hazard. The measuring-instrument frame
still fits (the fix is documentation + a missing pin), but the spec must not inherit the
"unreachable states" title claim, which is now disproven.

## §4 Design forks (human decides, one at a time)

**F1 — H38 fix shape.**
- A: direct unit pin on `classify_spell_heuristic` (1 test, `lsp_client.rs`).
- B: engine-level pin with real LT ids (`FR_SPELLING_RULE`, `EN_CONTRACTION_SPELLING`) in
  `ltex_ls.rs`'s existing classify test.
- C: A + B + a fixture-matrix agreement test between the shared fn and harper's private
  duplicate (in `harper_ls.rs`'s test mod).
**Recommendation: C.** It is ~30 lines, closes the filed hole with the kill condition, grounds
the pin in verified real inputs, and retires the adjacent "intentionally identical, untested"
gap in the same motion. If minimalism wins, B alone satisfies the filing (the kill condition
holds: deleting `lsp_client.rs:69` reddens an `FR_SPELLING_RULE → Spelling` assertion).

**F2 — H28 disposition.**
- A: keep both tests; rewrite their doc comments to the verified production-reachability story;
  add the Route-B pin(s) (unit on `classify_destination_enter` with Other/Unknown; optionally
  one intercept-driven unix test); file the behavioral tail as a new item.
- B: keep + re-document only (no new tests).
- C: the filed fix — retire tests + branch. (Refuted by grounding; listed for completeness.)
**Recommendation: A.** The unit pin is cheap and kills two birds (Route B + the `_` catch-all
absorption risk). C would have deleted production behavior — worth stating in the effort report
as the second consecutive item whose filed fix inverted under grounding.

**F3 — the H28 behavioral tail (destination-mode per-kind refusal wording).**
- A: file as a new backlog item (S-size UX polish), out of Batch T.
- B: fold into Batch T. (Breaks the "no shipped-behavior change" premise.)
- C: drop entirely — decide the "empty path" wording is good enough for a rare state.
**Recommendation: A.** It is a real, verified wording defect in a reachable state, but it is
behavior; the batch premise is worth more than the fix's urgency.

**F4 — H36 scope.**
- A: full mechanical sweep of the 98 join sites (exclusions per §2), sequenced LAST in the
  effort, after H28's lines settle.
- B: close H36 as not-worth-it, on the record.
- C: targeted sweep of the boilerplate-heavy files only.
**Recommendation: A, by a modest margin.** The grounding removed the risk argument (verified
no-decay, verified test-only, verified exclusion list), the batch was sequenced expecting this
volume, and A is the only option that actually finishes H32's "exactly one answer" goal. B is
honest and live — say the word and it closes cleanly with the rationale archived. C is the one I
would argue against (two idioms persist AND the churn happens anyway).

**F5 — the three pumped picker-seed bare sites (`prompts.rs:714,794,818`).**
- A: leave all bare sites untouched (pure exclusion list).
- B: additionally migrate these 3 to `scratch_dir()` seeds — deterministic listings, one
  clearly-labeled semantic commit; leaves H28's two seeds alone (their doc comments reason
  about `temp_dir()` explicitly and they never pump).
- C: also migrate H28's two (after F2's re-documentation rewrites those comments anyway).
**Recommendation: B.** It converts the only latently-exposed tests to hermetic listings for ~3
lines each; H28's pair stays put because their non-pumping makes the dir semantically inert and
F2-A already rewrites their comments to say exactly that.

## §5 What the controller's sweep missed

1. **`swap.rs:48`** — the one `temp_dir()` site in production-function code (`state_dir()`'s
   `#[cfg(test)]` branch) with per-process STABILITY semantics a naive sweep would break (1.7).
2. **The `Nothing` arm's quit-drain abort** (`file_browser_commit.rs:373-377`) — the filed
   "retire the dead branch" would have touched quit semantics, not just a status string (1.2).
3. **The branch is reachable in plain English usage** (`EN_CONTRACTION_SPELLING`,
   `OXFORD_SPELLING_*`), so H38's pin is not defending an exotic-locale corner (1.1).
4. **3 of the brief's "17 bare" sites are comment lines and 2 are line-wrapped joins** — the
   real bare population is 12, one of which (`swap.rs:682`) is un-sweepable on semantics (1.3).
5. **The kind-based `Nothing` fall-through is completely unpinned** while the `None`-highlight
   case already has a unit test — the precise, smallest missing test H28 should add (1.11).
6. **The H28 prose's "unreachable once a listing lands" analysis only modeled the default
   `".."` highlight** — the navigated Other/Unknown case was never considered by either the
   filing or the effort-① re-grounding (1.2, Route B).
7. Housekeeping when the batch ships: the H28 archive entry must carry the correction (its
   central mechanism claim is disproven), and H36's stale per-file counts should be restated
   from the measured numbers (1.3) so the archive doesn't preserve wrong figures.
