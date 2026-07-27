# Batch T — binding decisions

Human decisions on the forks in `fable-grounding.md` §4. These are BINDING on the spec, the plan,
and every implementer. A finding that contradicts one of these is a HUMAN decision, not a fix.

---

## D1 (F1) — H38 fix shape: option C

Pin `classify_spell_heuristic`'s code-substring branch **three ways**:

1. A direct unit pin on `classify_spell_heuristic` itself (`lsp_client.rs` test mod).
2. An engine-level pin through `LtexEngine::classify` using the **real LanguageTool rule ids
   verified from the installed jars** — `FR_SPELLING_RULE` and `EN_CONTRACTION_SPELLING`, both of
   which contain `SPELL` yet miss all three of the `MORFOLOGIK`/`HUNSPELL`/`SPELLER` short-circuits
   (grounding §1.1). Extend `ltex_ls.rs`'s existing classify test.
3. A fixture-matrix **agreement test** between the shared `classify_spell_heuristic` and harper's
   private duplicate `harper_ls::classify_lsp`, retiring the untested "the two bodies are
   intentionally identical" claim in `lsp_client.rs`'s doc comment.

**Kill condition, stated explicitly in the spec and carried into the plan:** deleting the code
branch at `lsp_client.rs::classify_spell_heuristic` (the `code.to_lowercase().contains("spell")`
early return) MUST redden at least one test in each of (1) and (2). Each implementer verifies this
by mutation — break it, watch it redden, restore, confirm with `git diff` — not by reading.

Rationale for C over the minimal B: the duplicate is pinned and the shared original is not, and the
doc comment asserting they are identical is itself untested — the same defect shape H38 exists to
fix, one line away in the same comment. Closing both in one motion costs ~30 lines.

---

## D2 (F2) — H28 disposition: option A — keep, re-document, pin Route B

**Both tests STAY.** `save_as_empty_path_is_a_sticky_warning` and
`block_write_empty_path_is_a_sticky_warning` (`prompts.rs`) are not asserting an unreachable state;
they assert the **pre-listing async window**, which is a real production state by design
(grounding §1.2 Route A). Their non-pumping is CORRECT and load-bearing, not an oversight.

Three obligations:

1. **Rewrite both doc comments** to the verified reachability story. The current comments record the
   reverted pump experiment and imply the asserted state is an artifact; that is now disproven. The
   new comments must say what the test pins (the pre-listing window), why it must NOT pump (pumping
   moves it to a different, already-covered state), and that the state is production-reachable.
2. **Add the Route-B pin** — the precise missing test (grounding §1.11): a unit test on
   `file_browser_commit::classify_destination_enter` with an empty field and a highlighted entry
   whose kind is `Other`/`Unknown`, asserting `CommitOutcome::Nothing`. The `None`-highlight path
   into `Nothing` is ALREADY unit-pinned; the kind-based fall-through is not pinned at all. This
   also guards the `_ =>` catch-all against silently absorbing a future `EntryKind` variant.
3. **Do NOT delete the `Nothing` arm or any branch.** Beyond the reachable warning, that arm also
   ABORTS an in-progress quit drain (`file_browser_commit.rs`, the Effort-6 Codex-C2 rule), so the
   filed "remove the dead branch" would have changed quit semantics (grounding §1.2, §5.2).

The H28 title ("Un-pumped picker tests assert unreachable states") and its prose's central mechanism
claim ("the empty-path warning is genuinely unreachable once a listing lands") are **DISPROVEN**.
The spec must not inherit them, and the archive entry must carry the correction when the batch ships
(grounding §5.7) so the wrong figure is not preserved as history.

**Process note for the effort report:** this is the second consecutive item whose FILED FIX inverted
under grounding. A deferred item's stated fix is its least reliable content — it is written at the
moment of deferral, when the deferrer knows least. Ground the fix, not just the dependency.

---

## D3 (F3) — the H28 behavioral tail: option A — file it OUT of Batch T

In Route B (a navigated `Other`/`Unknown` highlight with a landed listing and an empty field) the
writer is told **"save-as: empty path"**, but the field's emptiness is not why the commit refused —
the highlighted entry is not a writable regular file. Select mode already distinguishes these
(`file_browser::classify_enter` has per-kind `EnterOutcome::Refuse` wording for broken symlinks,
fifos, devices); the destination-commit path has no counterpart.

**Real, verified, and NOT fixed in Batch T.** File as a new backlog item (~S, UX polish) before the
effort's implementation begins, so the finding is durable rather than living in a scratchpad.

Rationale: the fix is small and we are already in the file, which is exactly the temptation to
refuse. It changes a **user-visible status string** — a shipped-behavior change. Batch T's entire
premise is "nothing here can change shipped behavior," and that premise is what licenses reviewing
the 97-site sweep as safe volume rather than as 97 opportunities to alter something. Spending the
premise on wording for a rare state is a bad trade. Option C (drop it) undersells a state that is
genuinely reachable with a message that is genuinely wrong.

---

## D4 (F4) — H36 scope: option A — the full sweep, sequenced LAST

Sweep all **97** `temp_dir().join(...)` scratch-path constructions onto
`test_support::{scratch_path, scratch_dir}`, sequenced **last in the effort**, after H28's lines
settle (H28 and H36 collide on `prompts.rs:502,522` — grounding §1.6).

> **AMENDED 2026-07-27, count only: 98 → 97.** As first written this decision said "98", a figure
> inherited from the grounding pass. It is arithmetically incompatible with the exclusion list this
> same decision binds: the tree holds 101 join constructions (99 same-line + 2 line-wrapped at
> `prompts.rs:1263,1383`), and removing the 4 binding exclusions leaves **97**. The 98 figure
> counted `swap.rs:48` as sweepable while the exclusion list below forbids touching it — an
> implementer could not satisfy both. Verified independently three times (controller, spec author,
> Codex spec gate rounds 1 and 2), each deriving 97 from the tree.
>
> **The DECISION is unchanged** — option A, "the full sweep with the binding exclusion list." Only
> the descriptive count is corrected to match the scope the decision already fixed. The complete
> partition of the 116 `temp_dir()` hits is `97 swept + 4 excluded joins + 12 bare + 3 comments`.
> The sweep touches **25 files** (of 29 that contain any hit).

**Exclusion list — BINDING, do not sweep these:**
- The **12 bare `temp_dir()` sites** (no `.join`): the directory itself is the subject, not a
  scratch path. Substituting `scratch_dir()` swaps a shared, populated dir with a parent (so its
  listing carries a `".."` row) for an empty freshly-created one — a semantic change to the test,
  not a delegation. (Grounding §1.3. Subject to D5.)
- **`swap.rs:48`** — a `#[cfg(test)]` branch inside the PRODUCTION `state_dir()` function, with
  per-process stability semantics a counter-based seam would break (grounding §1.7, §5.1).
- **`tests/harper_ls_probe.rs`, `tests/harper_ls_integration.rs`** — a separate crate; cannot reach
  a `#[cfg(test)] pub(crate)` seam at all.
- **`test_support.rs`'s own `scratch_name`** — the seam's implementation.

**Do NOT add a textual scanner** banning raw `temp_dir().join(...)`. Same reasoning as H32
(H31 fork 3 / effort ① D5): the `fs_chokepoint` scanner was measured to leave 5 of 6 evasion routes
uncaught, so a second scanner is self-defeating. This is mechanical delegation, not a new gate.

Rationale, and the honest shape of the call: the grounding **removed the risk argument** that
deferred this out of H32 (verified test-only, pid-unique, zero duplicate labels, no decay) — but the
same grounding also shrank the **value** argument, since sites with no decay and no collisions mean
the payoff is uniformity, not correctness. A wins on one point Fable understated: leaving 97 sites
on the old idiom means the next reader sees two ways to get a scratch path and no signal which is
current — which is precisely how a seam decays back into being one option among several. Option B
(close as not-worth-it) was live and defensible. Option C is rejected: it leaves both idioms in the
tree AND spends the churn.

**Review hazard to carry into the plan:** three separate efforts have now had review findings hide
inside mechanical-looking diffs. A 97-site sweep is the highest-risk possible shape for that. The
plan must make the sweep verifiable by something other than reading it — see the spec's
verification strategy.

---

## D5 (F5) — the three PUMPED bare sites: option B — migrate exactly those three

`prompts.rs:714`, `:794`, `:818` are the exception to D4's bare-site exclusion. Unlike the other
nine, these tests **pump the listing and then act on it**, so they read whatever the shared system
temp dir happens to contain — one already carries a comment about gating "regardless of whatever
`temp_dir()` happens to sort first," i.e. a test defending itself against content it does not own.

**Migrate these three to `scratch_dir()` seeds** for deterministic, hermetic listings. Ship as ONE
clearly-labeled **semantic** commit, kept separate from D4's mechanical sweep — a semantic change
must not hide inside 97 mechanical ones.

**H28's two seeds (`prompts.rs:502`, `:522`) stay bare** (option C rejected). They never pump, which
makes the directory semantically inert to them — they never read its contents at all, so migrating
churns a line to no effect. Worse, their `temp_dir()` is load-bearing for the D2 doc-comment
rewrite: those comments must reason about why the shared dir's `".."` row would matter IF the test
pumped. Swapping in an empty scratch dir would leave the rewritten comments describing a directory
the test no longer uses.

Option A (leave all 12 alone) was the conservative call and cheap, since the exposure is latent
rather than active; B wins because ~3 lines each retires the only latent flakiness surface the
grounding found anywhere in the population.

---

# Summary of binding decisions

| id | fork | decision |
|----|------|----------|
| D1 | F1 | H38: pin three ways (unit + real-LT-id engine pin + harper-agreement matrix); kill condition mutation-verified |
| D2 | F2 | H28: keep both tests, re-document to the verified reachability story, add the Route-B kind-based pin; delete nothing |
| D3 | F3 | H28's behavioral tail (destination-mode per-kind refusal wording): FILE OUT as a new item, do not fix in-batch |
| D4 | F4 | H36: full **97**-site sweep (amended from 98 — see D4), sequenced LAST, with the binding exclusion list; no textual scanner |
| D5 | F5 | The 3 PUMPED bare sites migrate to `scratch_dir()` as one separate semantic commit; H28's 2 stay bare |

**Effort shape:** ONE effort, internal order **H38 → H28 → H36-sweep-last** (H28 and H36 collide on
`prompts.rs:502,522`).

**The premise that must survive to merge:** *nothing in Batch T changes shipped behavior.* D3
protects it. Any finding that would change a user-visible string, a status kind, or a control-flow
branch is out of scope and gets filed, not fixed.
