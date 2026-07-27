# A22 — design decisions

Forks resolved with the human, one at a time. Grounding: `scratchpad/a22/fable-grounding.md`.
Brief: `scratchpad/a22/grounding-brief.md`.

---

## D1 (Fable F1) — What does the Write-Block → Export redirect mean for scope?

**DECIDED: (b) Honour the marked block, with (a)'s disclosure chrome folded in.**

Not alternatives: B keeps the mode's promise end-to-end, A's chrome makes the scope legible on
the way there. Rejected: (c) refuse — reverts C5's "advice with somewhere to go" and leaves a
writer who wants this block as `.docx` with no path; (d) confirm-then-whole-document — consent
without capability, and it re-creates the half-cleared-pair hazard `prompts.rs`'s M6 comment
warns about.

Shape (Fable B1, on the grounding's evidence):
- `DestinationPurpose::Export { ext, scope }` with `enum ExportScope { WholeDocument, MarkedBlock }`.
- Scope is a **FLAG re-read at dispatch, never stored offsets** — background merges
  (`FilterDone`/`TransformDone`/`JobDone`) are deliberately still processed while the picker is
  open and legally edit buffers; the `Buffer::apply` funnel (H22) remaps `marked_block`, so
  snapshotted offsets would go stale. Matches Write-Block's existing commit-time and
  confirm-time re-reads.
- Rides boundary 1 (async picker listing) on the existing `BrowseMode::Destination.purpose`;
  rides boundary 2 (overwrite confirm) on a new `PendingExport.scope` field.
- Derived at `redirect_to_export`: `WriteBlock → MarkedBlock`, `SaveAs → WholeDocument`.
  `run_export_with_probe` always `WholeDocument` — the four palette export commands stay
  whole-document.

Cost accepted: **M**, not the S the controller's own sweep estimated. ~2 production constructs,
2–3 production patterns, ~6 test sites, all compiler-forced.

Consistent with the re-kind of A22 from `feature` to `bug` (2026-07-27): the other options
re-label or amputate the wrong artifact rather than remove it.

**Engineering calls this inherits (decided by evidence in the grounding, not forks):**
- `hidden` is ignored — display-only; `perform_block_write` already ignores it.
- `apply_export_done` needs no scope logic — finalization is target-and-bytes only.
- The two `redirect_to_export` call sites are the complete fix surface.

---

## D2 (Fable F2) — Block gone at dispatch: refuse, or fall back to whole-document?

**DECIDED: (a) Refuse.** Status "no marked block — export cancelled"; nothing is written.

Applies at BOTH dispatch moments: the commit arm and the post-`OverwriteExport` confirm.

Reachable, not theoretical: a background merge can collapse the block mid-flow (the funnel
clears a collapsed block) and a plugin can drive `undo` through the pump.

Rejected: (b) fall back with a disclosure status — that is A22's own bug shape one layer down,
a block-scoped flow silently producing a whole-document file. Fixing that pattern at the
redirect and reintroducing it at the dispatch would be incoherent.

Parity is the deciding argument: `commit_destination`'s WriteBlock arm and
`PromptAction::OverwriteWriteBlock` both already refuse with "no marked block". One rule, not two.

Accepted cost: `commit_destination` sets `editor.file_browser = None` BEFORE the purpose
dispatch, so a refusal leaves the picker closed and the flow dead — the writer restarts from
^KW and retypes the destination. Pre-existing ordering, inherited not introduced.

---

## D3 (Fable F3) — How loudly does scope show in chrome?

**DECIDED: all four surfaces.**

1. **Redirect status** — "…opening Export for the marked block". Free; the string exists and
   `redirect_to_export` already receives `purpose`.
2. **Picker footer** (`file_browser::footer_target`, `ExtVerdict::Redirect` arm) — names the
   scope while the writer is still typing. One extra `matches!`; the footer can already reach
   the purpose via `fb.mode`. THE only pre-commit surface.
3. **Picker title** (`render_overlays.rs`, the `Export` arm) — " Export .docx (marked block)
   to: … ". Free once scope rides the purpose; persistent through destination choice.
4. **Completion status** — "exported block to {target}" — ACCEPTED despite being the only one
   with structural cost: `apply_export_done` is target-and-bytes and never sees scope, so
   `Msg::ExportDone` gains a scope field (ONE production construct; both dispatch sites already
   use `..`, so nothing else breaks).

Reasoning for 4 specifically: the whole item is "a writer walks away with the wrong file", so
the line that states which file they got is worth a message field. Accepted trade — this is the
one place scope leaks from the flow layer into the job-message layer.

C5 lesson binds all four: **test the rendered screen, not the struct.**
The `app.rs` ExportDone test asserting only `contains("exported")` will not constrain the new
wording — it needs a fixture that fails if scope is dropped.

---

## D4 (Fable F4) — Effort scope: which adjacent findings ride along?

**DECIDED: fold (ii), (iii), (iv). File (i) separately.**

**IN SCOPE:**
- **(ii) redirect skips the pandoc probe** — `redirect_to_export` opens an Export picker without
  `run_export_with_probe`'s gates. Add the `probe_pandoc()` check at the redirect site: an
  immediate honest refusal instead of a late worker-thread subprocess error. (The saved-source
  gate is NOT added — `do_export` feeds pandoc from stdin and never needs `document.path`.)
- **(iii) `cancel_destination` asymmetry** — clears `pending_write_block` but not
  `pending_export`. Not a live bug (both are only set after the picker closes, so neither clear
  is load-bearing), but unexplained. One-line tidy in a file the effort already opens.
- **(iv) plugin-pump active-buffer switch** — `pump()` runs every loop iteration with no overlay
  guard and `wc.command` can dispatch `next_buffer` while the picker or confirm is open, so a
  later `active()` read can land on a different buffer. Capture `BufferId` at picker-open,
  refuse on mismatch at dispatch. Closes it for Write-Block in the same stroke.

  *Why folded despite Fable listing it as separable:* D1 adds a SECOND consumer of "re-read the
  mark at dispatch" and D2 makes a refusal path depend on that read being trustworthy. Shipping
  a new dependent of a read known to be able to hit the wrong buffer would leave the effort's own
  correctness argument holed. Same shape as the rest of the design: capture the flow's context at
  open, verify at dispatch.

**OUT OF SCOPE — filed separately:**
- **(i) typed-vs-highlighted foreign-extension asymmetry.** Highlighting an existing `.rtf` is
  refused (`HighlightVerdict::Foreign` — "saving markdown over it would destroy it"); TYPING
  `notes.rtf` is honoured and silently written as markdown bytes under the foreign name. Same
  destruction, opposite answer, decided only by how the file was named.

  Excluded as a different QUESTION, not as unrelated work: A22 asks "does scope survive the
  redirect"; (i) asks "when may we write markdown over a foreign file" — a refusal-policy fork
  with its own product answer, on a path that involves no blocks. Folding it would put two
  unrelated deliberations in one gate.

---

## D5 (Codex spec gate round 1, Important) — Law 10 vs the N/A conformance claim

**DECIDED: (c) Keep N/A, strengthen the argument on evidence, re-run the gate.**

Codex round 1 returned `not ready`: §9's N/A conclusion contradicted contract law 10, since the
spec's own wording conceded "a capability reachable by NO command."

The concession was the DRAFT'S error, not the design's. Verified fact neither the spec nor Codex
had in view: **`block_write` is a registered command** (`registry.rs:435`, `MenuCategory::Block`,
also ^KW). A plugin can already dispatch it; it opens the Write-Block picker; a pandoc-producible
extension there is exactly the path this effort makes block-scoped.

Parity that settles it: the four `export_*` commands do NOT give a plugin parameterized export
either — `run_export` opens a picker, and law 10 states commands stay nullary today. So
whole-document and block-scoped export are EQUALLY plugin-reachable, by construction.

Rejected: (a) add `export_block_*` commands — that is Option E, excluded at D1/F1, and it would
add surface to satisfy a breach that does not exist; (b) amend the contract — amending project
law to excuse something that was never a violation.

§9 rewritten with §9.1 answering law 10 directly, including a review note recording that Codex's
finding was valid against the text it reviewed and is answered on evidence.

**Standing caveat accepted by the human:** if Codex holds its finding after seeing the
`block_write` evidence, that is real signal — return to the human rather than loop.
