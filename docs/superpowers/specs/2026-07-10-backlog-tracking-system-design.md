# Backlog tracking system — design

**Date:** 2026-07-10
**Status:** design — awaiting user review before implementation
**Scope:** repo tooling + docs migration (NOT app code). Right-sized: brainstorm → this
design doc → direct implementation, gated by `cargo test` + clippy. The heavy Codex/Fable
review layers are for editor-correctness work and are **not** applied here.

---

## 1. Problem

Work is tracked across two prose docs — `docs/ux-backlog.md` (1,515 lines) and
`docs/engineering-health.md` (355 lines) — plus `CHANGELOG.md` (release notes) and the
git-local SDD execution ledger (`.git/sdd/progress.md`). The two backlog docs are the
"issue tracker," and they have four structural problems:

1. **Status is prose, hand-maintained, and triplicated.** Each item's header, the "Sizing
   summary," and the "Working order" section each restate status independently, so they
   disagree. Confirmed drift: **E6 (splash) is marked `needs-design` in its header but
   actually shipped** (merge `242c987`); the same was true of other splash-era work.
2. **ID-namespace collision.** "Theme H" exists in *both* docs with *different* meanings:
   ux-backlog `H1` = the 2026-07-04 app.rs leaf-extraction; engineering-health `H1` = the
   2026-07-09 god-object SEAM refactor. Same label, different items.
3. **Surfacing open-vs-done is an expensive read** — you must scan two multi-thousand-line
   files that can contradict each other.
4. **Shipped history drowns open work** — ~80% of `ux-backlog.md` is retained SHIPPED
   prose; the ~20 open items are buried in it.

The three roles are genuinely distinct — release notes (CHANGELOG), the work backlog (the
two docs), and the in-flight execution ledger (SDD). The fix is **one queryable index over
the backlog**, not merging everything into one bucket.

## 2. Goals / non-goals

**Goals**
- A single structured source of truth for each item's *state*, so status cannot drift.
- Surface "all open work" and "all completed work" from **one small file** — no expensive
  LLM/human reads across multiple large prose docs.
- Make staleness a **merge-gate failure** (drift-proof, not drift-resistant), reusing the
  existing `cargo test` gate culture (`module_budgets.rs` is the model).
- Keep the rich, source-grounded triage prose as-is (its own strength) — only remove *status*
  from it.
- Integrate the `bl:` capture shortcut natively.

**Non-goals**
- Not replacing `CHANGELOG.md` (release notes — a different axis) or the SDD ledger (per-task
  execution — git-local, ephemeral). They stay; the manifest links to them.
- Not adopting an external tracker or in-repo issue tool (rejected: fragments grounded prose,
  worse for agent handoff).
- Not backfilling the shipped hardening-campaign history (M1–M7 etc.) from `CLAUDE.md` — that
  narrative stays in `CLAUDE.md`; only the *open* hardening items migrate (see §9).

## 3. Architecture

Two files, **both at repo root**, beside `CHANGELOG.md` and `CLAUDE.md` (the backlog is a
top-level project-status concern; the source and its generated view are tightly coupled and
belong together — splitting a hand-edited source from its generated output across directories
recreates the very "two places that must stay in sync but live apart" problem we are killing):

- **`backlog.toml`** — the *sole* source of truth for item **state**.
- **`BACKLOG.md`** — the human/LLM-facing dashboard, **generated** from `backlog.toml`,
  checked in. This is the one file read to know the whole picture.

The rich triage **prose** stays in `docs/ux-backlog.md` and `docs/engineering-health.md`,
keyed by item ID — but with **status tokens stripped from the headers** (status now lives only
in `backlog.toml`). The prose is read only when actually working an item.

```
backlog.toml  ──(pure render fn)──►  BACKLOG.md         (dashboard: open + shipped log)
     │                                                   
     └──(doc anchor per item)──►  docs/ux-backlog.md      (triage prose, status-free headers)
                                  docs/engineering-health.md
```

## 4. Schema — `backlog.toml`

One `[[item]]` table per entry (open and terminal alike):

```toml
[[item]]
id      = "A9"                 # globally unique across the whole backlog
title   = "Wrap Column state-in-label"
status  = "needs-design"       # enum, §4.1
kind    = "feature"            # enum, §4.2
size    = "S"                  # enum, §4.3
theme   = "A"                  # enum, §4.4 — groups the dashboard
hook    = "Rename 'Set Wrap Column…' → stateful 'Wrap Column: <value>'."
doc     = "ux-backlog.md#a9"   # anchor into the triage prose (file#slug)
blocks_effort_p = false        # true → must clear before the Effort-P capstone
depends_on = []                # list of ids (ordering / prerequisites)
created = "2026-07-09"         # capture/first-filed date (YYYY-MM-DD)
# terminal-only fields (required by status, §4.1):
#   shipped_commit = "d7a5494"
#   shipped_date   = "2026-07-07"
#   dropped_reason = "…"
```

### 4.1 `status` (lifecycle) — the only place status is recorded

| value          | meaning                                                        | required extra fields          |
|----------------|---------------------------------------------------------------|--------------------------------|
| `triage`       | freshly captured (via `bl:`), not yet analyzed                | —                              |
| `needs-design` | analyzed; design forks remain                                 | —                              |
| `ready`        | design settled; can be specced/built                          | —                              |
| `in-progress`  | an effort is actively underway (see the SDD ledger)           | —                              |
| `watch`        | filed, **not a defect**; act only on a named trigger          | —                              |
| `shipped`      | terminal — done                                               | `shipped_commit`, `shipped_date` |
| `dropped`      | terminal — decided against                                    | `dropped_reason`               |

`triage` is the new state that makes `bl:` pay off: "what raw ideas have I captured but not yet
thought through?" is a one-field filter, distinct from designed-but-unbuilt work.

**Atomic-status rule (important):** an item has exactly one status. **Partial completion is
modeled by splitting the item**, never by a half-status. (This is why H1 splits — §9.1.)

### 4.2 `kind`
`feature` | `bug` | `debt` | `chore` | `research`

### 4.3 `size`
`S` | `SM` | `M` | `L` | `XL` | `TBD`  (`TBD` is the default for `triage` captures)

### 4.4 `theme` (validated set)
`A` command-surface · `B` rendering · `C` document workflow · `D` config/persistence ·
`E` product identity/chrome · `H` code-health / engineering-health · `M` hardening ·
`R` responsiveness · `S` manuscript structure · `P` plugins (Effort-P capstone + candidates).

## 5. Tooling

No new language, no new dependency (`toml 0.8` already present).

### 5.1 The gate — `wordcartel/tests/backlog.rs`
A `#[test]` run by `cargo test` (already a merge gate), sibling to `module_budgets.rs`. One
pure `render(&Manifest) -> String` function is used for **both** generation and verification, so
they cannot diverge. The test:

1. **Parses** `backlog.toml` (the `toml` crate) into typed structs; unknown fields / bad enum
   values fail with a clear message.
2. **Validates invariants** (§7).
3. **Renders** the expected `BACKLOG.md` via `render()` and asserts it byte-equals the committed
   file; on mismatch it prints "run `BLESS_BACKLOG=1 cargo test --test backlog` to regenerate."
4. When `BLESS_BACKLOG=1` is set, it **writes** `BACKLOG.md` instead of asserting.

### 5.2 Convenience wrapper — `scripts/backlog`
Thin shell over the test + TOML, for low-friction human/agent use:
- `scripts/backlog bless` — regenerate `BACKLOG.md` (`BLESS_BACKLOG=1 cargo test --test backlog`).
- `scripts/backlog add` — scaffold a well-formed `[[item]]` (mint next ID in a theme, template
  the required fields) so neither a human nor an agent hand-malforms TOML.
- `scripts/backlog open` / `scripts/backlog shipped` — instant filtered views (grep the rendered
  dashboard) without opening a file.

## 6. `bl:` integration

`bl: <thing>` in the new system:
1. Mint the next unique ID in the appropriate theme.
2. **Light grounding** — a quick grep/read to pin source anchors (unchanged behavior).
3. Append an `[[item]]` to `backlog.toml` with `status = "triage"`, `size = "TBD"`, a one-line
   `hook`, `created`, and the `doc` anchor.
4. Append the prose triage stub under that anchor in `ux-backlog.md` or `engineering-health.md`.
5. **Regenerate `BACKLOG.md`** (so the working tree stays self-consistent and the gate stays
   green even while uncommitted).
6. **Leave uncommitted by default** (unchanged).

Memory `[[backlog-shorthand-bl]]` is updated to describe this flow (file to `backlog.toml` as a
`triage` item + prose stub + regenerate; still uncommitted by default).

## 7. Drift-resistance invariants (enforced by the gate)

The test fails the build unless all hold:
- **I1 — IDs globally unique.** No two items share an `id`.
- **I2 — Enums valid.** `status`/`kind`/`size`/`theme` are in their allowed sets.
- **I3 — Terminal completeness.** `shipped` ⇒ `shipped_commit` + `shipped_date`; `dropped` ⇒
  `dropped_reason`.
- **I4 — Anchor cross-reference (bidirectional).** Every item's `doc` anchor resolves to a real
  heading in the named prose file, and every backlog-item heading in those files maps to exactly
  one manifest `id`. (Catches orphaned prose and orphaned rows — the E6 class.)
- **I5 — No status in prose headers.** The prose-doc item headers contain no status token
  (`SHIPPED`/`needs-design`/…) — status is manifest-only, so it cannot be double-sourced.
- **I6 — `depends_on` resolves.** Every referenced id exists (no dangling deps).
- **I7 — `BACKLOG.md` up to date.** Committed file byte-equals `render(manifest)`.

## 8. Generated `BACKLOG.md` format

The cheap-read entry point. Deterministic ordering (so the golden test is stable):

1. **Dashboard header** — generated counts: total open, by `status`, by `size`, and a
   `blocks_effort_p` tally.
2. **Open table** — every non-terminal item, sorted by `(blocks_effort_p desc, size, theme, id)`:
   `id | title | status | kind | size | P? | hook | doc-link`.
3. **Shipped log** (collapsed `<details>`) — `id | title | shipped_date | commit`, newest first.
4. **Dropped** (collapsed) — `id | title | reason`.

Everything is generated; the hand-maintained "Sizing summary" and "Working order" sections in
`ux-backlog.md` are **deleted** (now derived).

## 9. ID namespace + full rename/merge/reconcile table

Every existing entry migrates 1:1 **keeping its current id** except the rows below. IDs are
globally unique across both docs; the two `H` collisions and the one partial-ship are resolved
here.

### 9.1 Splits (partial completion → atomic items)

| Current | Becomes | Notes |
|---|---|---|
| eng-health `H1` "Decompose the two god-objects · PARTIALLY SHIPPED" | **`H1`** god-object SEAM decomposition → `shipped` (`304e263`, 2026-07-09) **+** **`H14`** "`render()` body split by paint surface" → open (`ready`, `M`, `blocks_effort_p=true`, `depends_on` relates `H9`/`H11`) | H1's `hook` records "SEAM shipped; render() split spun out to H14." |
| ux-backlog `A6` (shipped) + its "overlay mouse parity" follow-up | **`A6`** palette reachability → `shipped` (2026-07-04) **+** **`A13`** "overlay mouse parity (theme-picker/file-browser click-to-select, outline click-to-jump)" → open (`needs-design`, `A`, `SM`) | The follow-up was already tracked in-prose as a distinct open sub-item. |

### 9.2 Renames (resolve the `H` collision — one code-health `H*` namespace, owned by eng-health)

| Current | Becomes | Item |
|---|---|---|
| ux-backlog `H1` (Theme H) | **`H15`** | app.rs/render.rs leaf extraction (first pass) → `shipped` 2026-07-04 |
| ux-backlog `H2` (Theme H) | **`H16`** | `active_line` end-of-buffer clamp → `shipped` (`0573eec`, 2026-07-08) |

(Prose for `H15`/`H16` relocates from `ux-backlog.md` into `engineering-health.md` so the `H*`
namespace lives in one file; their `doc` anchors point there.)

### 9.3 Status reconciliations (drift fixes surfaced during audit)

| Item | Old header says | Correct status |
|---|---|---|
| `E6` splash / start screen | `needs-design` | **`shipped`** (`242c987`, 2026-07-09) |

### 9.4 New items captured from `CLAUDE.md` open hardening list (theme `M`)

| id | title | status | size |
|---|---|---|---|
| `M8`  | M5 follow-up — undo "louder hint" for buffer-level merges | `ready` | `S` |
| `M9`  | Optional: upgrade/patch `pulldown-cmark` (M4-rest only isolates its parse panic) | `watch` | `S` |

(The "deep incremental-soundness tail" hardening item is **already** represented by eng-health
`H3`, status `watch` — no duplicate is created.)

### 9.5 Effort-P cluster (theme `P`)
`P` (the Effort-P plugin capstone, `needs-design`, `XL`, anchored to
`docs/design/effort-p-plugin-system-design-space.md`) plus the candidate clusters `PA`/`PB`/`PC`
(`research`/`watch`, post-P), migrated from ux-backlog Theme P.

*(The complete transcribed manifest — every item, not only the changed rows — is produced during
migration and reviewed as part of the resulting diff.)*

## 10. Scope boundaries (what folds in vs stays put)

- **In:** every entry in `ux-backlog.md` + `engineering-health.md`; the 3 open hardening items
  from `CLAUDE.md` (§9.4); the Effort-P cluster.
- **Stays put:** `CHANGELOG.md` (release notes); the SDD ledger `.git/sdd/progress.md` (per-task
  execution, git-local — `in-progress` items *link* to it, it is not absorbed);
  `docs/superpowers/{specs,plans,reviews}/` (per-effort artifacts); `memory/` (decisions);
  `CLAUDE.md`'s shipped hardening narrative (M1–M7 etc. — not backfilled).

## 11. Migration plan (phases)

1. **Author `backlog.toml`** — transcribe all entries per §4/§9; assign each a `doc` anchor.
2. **Add the gate** — `wordcartel/tests/backlog.rs` (render + validate + bless) and
   `scripts/backlog`.
3. **Generate `BACKLOG.md`** (`bless`); confirm the dashboard reads correctly.
4. **Strip status tokens from prose headers** (I5) and add stable heading slugs/anchors (I4);
   relocate `H15`/`H16` prose into `engineering-health.md`; **delete** the "Sizing summary" +
   "Working order" sections.
5. **Green the gate** — `cargo test --test backlog` passes (all of I1–I7); workspace clippy clean.
6. **Update docs** — CLAUDE.md gains a one-line pointer to `BACKLOG.md` + the `bl:` flow; memory
   `[[backlog-shorthand-bl]]` updated.
7. **(Phase 2, optional — not required for the gate):** move shipped triage prose to a
   `docs/backlog-archive.md` to slim the live docs to open work only.

## 12. Testing

- The gate test itself is the primary test surface (§5.1, §7). It runs under the existing
  `cargo test` merge gate.
- A focused unit test on `render()` (a tiny fixture manifest → expected markdown) pins the
  dashboard format independently of the real data.
- `scripts/backlog` verified by running `bless` and confirming a clean re-generation is a no-op
  (idempotent).

## 13. Open decisions for user review
- Repo-root placement of both files (§3) — **confirmed** in brainstorm.
- Whether to do Phase 2 archival now or defer (§11.7) — **defer** recommended.
- The exact `M8`/`M9`/`PA`/`PB`/`PC` ids and the `H15`/`H16` relocation — confirm during the
  manifest-diff review.
