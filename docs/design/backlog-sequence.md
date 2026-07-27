# Backlog sequence — the agreed order of work

**Status:** SEQUENCE (2026-07-27), agreed with the human. Nothing here changes item state — status
lives only in `backlog.toml`. This records **what order to do the open work in, and why**, plus the
corrections that drive that order.

Supersedes the sequencing half of [`backlog-integration-relationships.md`](backlog-integration-relationships.md)
(2026-07-13), whose relationship map remains useful; several of its premises have since shipped.

**Provenance.** Two independent whole-backlog passes — one by the controller, one by Fable, written
without sight of each other — then compared and merged. All three documents:
`scratchpad/backlog-sequence/{controller-sequence,fable-sequence,comparison}.md`.

---

## Corrections that drive the order — read these first

These changed the sequence more than any prioritization argument did.

1. **E16 does NOT need PD.** `filter::run_subprocess` is the shipped one-shot subprocess core —
   argv, shell flag, **stdin as a `String`**, timeout, output cap, cancel flag, `ReapGuard` cleanup,
   off-thread on the jobs substrate. `export.rs` (~:211, ~:227) already drives pandoc through it.
   E16 is a **core Rust provider** and calls it directly; **PD is the plugin-facing primitive** (Lua
   marshaling, `!Send` pump-drain, security caps), which a core provider needs none of.
   **E16 is unblocked today.** Its hook's "fork 1" is not a fork.
   *How the error arose, because it generalizes:* the dependency was written into E16's hook on
   2026-07-26 from the design-space framing where vale-CLI was imagined as a **plugin** driver. When
   it became a core provider the dependency evaporated and the prose did not. **A hook is a claim,
   not a fact — including one written the previous day.**

2. **E12's "Needs wc.async (PD)" is wrong and always was.** An LSP client is a Rust worker thread;
   a Lua LSP client is a non-starter by the design doc's own §3. E12's real blockers are an
   **unfiled** plugin-dynamic-menu widening plus an actual demand signal. It stays parked — for the
   right reasons.

3. **A20 cannot be a plugin.** The Effort-P on-edit hook is **observer-tier**: a plugin structurally
   cannot veto a deletion. Forward-only drafting must gate the delete/cut command paths in **core**.
   PE's candidate list should drop A20.

4. **PE genuinely contains A18 and A19.** Filed apart, each would re-open the bundled-plugin
   question — a shared *decision*, not merely a shared mechanism. They are PE's content.

5. **A22 is mis-kinded.** Filed `feature`; it is a writer-facing **defect** — a block operation that
   silently exports the whole document.

6. **E8's "stays gated on S6/S8" is stale.** Both shipped; E8 is unblocked.

7. **H13 is effectively closed by A17 + H21** — the ~12 fields its own audit called real debt both
   shipped. One DRY nit remains (collapse the four prompt-payload fields into
   `Option<PromptPayload>`), which can ride any Editor-touching effort.

8. **H30 gets more reachable the moment E16 lands.** Today's spawns are rare (filters, export,
   session-start LSP children). E16 adds a one-shot spawn **per vale check** — per debounced edit —
   multiplying the fork windows. Its priority is coupled to E16's schedule.

---

## The sequence

| # | Work | Why here |
|---|---|---|
| 1 | **A22** — export honors the mark | The only open item a writer can hit today and walk away with a wrong file. Small; a live correctness defect in a shipped feature. |
| 2 | **Batch T** — H38 + H28 + H36 | Fix the measuring instrument before taking more measurements. Eleven could-not-fail tests surfaced in E11 alone and two more are filed. Nothing in this batch can change shipped behavior, so the ~105-site sweep is safe volume. |
| 3 | **H30** — CLOEXEC-safe pipes | Before E16 turns spawning from rare into per-check. Fixing an fd race *after* multiplying its windows is the wrong order. |
| 4 | **E16** — vale over the CLI | The largest writer-visible hole: a whole engine that has never once worked. Unblocked (correction 1), probe context fresh, live semantics matching harper and ltex. |
| 5 | **E14** — engine-side suppression | After E16, so the persistence decision is made **once** against the final engine set — vale resolves to permanently client-side, simplifying the scope. |
| 6 | **E8** — the lens model | Unblocked. Its model prices everything view-shaped after it (S9, S10, PE's lens exemplar, PA's proof-case slice); every deferral adds another surface to unify. |
| 7 | **H37** — the teardown race | After E16 settles the LSP message-class set, so the closing-flag protocol's per-class double-terminal analysis is done once rather than re-reasoned. Latent, so it trails the live-defect work. |
| 8 | **Batch W′** — A15 + H19 | Genuine smalls. |
| 9 | **B6** — heading-glyph style | On its own. Three open design forks plus command-surface gate churn and heading golden/pin churn across three styles — a `needs-design` item, not a papercut. |
| 10 | **PE ⊇ A18 + A19**; **A20 separately, in core** | The bundling mechanism plus two real features and the tutorial corpus. A20 is core input gating (correction 3). |
| 11 | **S10** → **Batch R** (B19 + H25) → **S3** → **S1** → **S2** | The remaining features in increasing size and dependency order; S2 last (`L`, wants the plugin story mature). |
| — | **S9** — floats by design | Its real dependency is accumulated user time in the ventilate lens. Scheduling it would fake that. |

**The one genuinely two-way call: 5 ↔ 6.** E14 first satisfies a recurring writer ask (permanently
silencing a rule you disagree with). E8 first captures the lens brainstorm while S6/S8 impressions
are fresh. Both passes independently flagged this as appetite-decided, not analysis-decided.

**Caveat on Batch R's placement:** B19 assumes wide-glyph text is not written here. If it is, B19 is
not display polish — it is explanations the writer cannot read — and it moves into the top third.

---

## Not doing

- **Park:** **E12** (both real prerequisites unbuilt, one unfiled, no plugin-authored engine exists
  or is asked for — anything sooner is speculative seam-building); **PD** (its proof case evaporated
  with correction 1; let PE pull it in when a bundled example actually wants a one-shot).
- **Close:** **H13** (correction 7); **H34** (0/470 across two concurrencies on clean main; the lone
  failure was on a deliberately-broken tree — close with a reopen-on-recurrence note).
- **Leave declined:** **H26** — its prose records the deliberate trade, and the guard's
  Caught/Not-caught contract already states its limit honestly.
- **Do not schedule:** **H35** — a speculative crate-wide newtype refactor with no motivating bug on
  file. File the first real byte-space confusion bug, then reconsider.
- **Keep as watch:** **M9**, **H3** (both hooks honest: act on a real CVE/parse bug or a real
  divergence sighting); **PA/PB/PC** (research placeholders; PA's readability slice stays subsumed by
  shipped S6 + S8 — at most one E8 proof-case plugin).

---

## The lesson worth keeping

Two independent passes over 33 items, and the highest-value output was **one grounded fact that
invalidated a fold** — E16's dependency, which existed only in prose written the day before. Neither
pass would have caught it by reasoning; it was caught by opening `filter.rs`.

**The backlog's stated dependencies are its least reliable content.** They are written at the moment
of deferral — when the deferrer knows least about what the item will actually become. Ground a
dependency before sequencing on it.
