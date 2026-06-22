# Wordcartel — Implementation Coverage Ledger

**Purpose:** trace every spec requirement to the effort (plan) that implements it,
so nothing falls through the cracks. Update the **Status** as efforts complete.

Spec: `docs/superpowers/specs/2026-06-21-wordcartel-design.md`

## Efforts (plans)

| # | Effort | Plan file | Produces | Status |
|---|---|---|---|---|
| 1 | **Edit Kernel** | `2026-06-22-wordcartel-01-edit-kernel.md` | pure buffer + ChangeSet undo + selection (headless lib) | ✅ COMPLETE (branch `effort-1-edit-kernel`, 28 tests green, final review READY-TO-MERGE + I-1/I-2/I-3 hardening) |
| 2 | **Render Core** | `2026-06-22-wordcartel-02-render-core.md` | md_parse (inline conceal+style) + layout/ColMap/cursor-nav (port spike) | IN PROGRESS |
| 3 | Incremental block_tree + block roles | _(next; needs block_tree spike)_ | CommonMark block invalidation (incremental==full oracle) + heading/list/quote rendering | PLANNED |
| 4 | IO / Shell | _(later)_ | crossterm input, ratatui render, clipboard, atomic save, filter, repar | PLANNED |
| 5 | App | _(later)_ | editor loop, commands, config, palette/menu, spellcheck, mouse | PLANNED |

> Effort 2 (Render Kernel, §16/§13/§9.2) split into Plan 2 (render core — inline
> conceal/style + the spike-validated layout/ColMap/cursor) and Plan 3 (incremental
> block_tree + block-role rendering). Seam: md_parse/layout take a line's **block
> role as input**; block_tree computes roles in Plan 3. Layout ports the validated
> spike at `~/projects/wordcartel-layout-spike`.

## Spec → effort map

Legend: ✅ done · 🔨 in this effort · ⏳ later effort · 📋 deferred to impl-spec detail.

| Spec § | Item | Effort | Status |
|---|---|---|---|
| 3.1 | Markdown-as-source-of-truth (`.md`, text not AST) | 1 (buffer holds text) / 3 (save) | 🔨/⏳ |
| 3.3 | Text not AST; plain-text edits | 1 | ✅ |
| 3.10, 16.1 | `ropey` buffer; **byte offset = canonical position** | 1 | ✅ |
| 9.1 | Undo = ChangeSet (retain/delete/insert) + branching history; `smartstring`; prose-tuned coalescing (~500 ms; break on paste / programmatic / cursor-move) | 1 | ✅ |
| 9.1, 3.6 | Selection = anchor/head over byte offsets; `SmallVec<[Range;1]>`; `.map(&ChangeSet)` | 1 | ✅ |
| 10.1 | Single mutation channel `apply(Transaction)`; selection mapped on the same atomic step | 1 (kernel `apply`) / 4 (wired loop) | 🔨/⏳ |
| 10.2 | `version: u64` revision token | 1 | 🔨 |
| 10.3 | O(1) rope snapshots for async workers | 1 (snapshot API) / 3 (workers) | 🔨/⏳ |
| 3.6, 9.5, 15.6 | In-process clipboard **register** (system sync is effort 3) | 1 (register) / 3 (system) | 🔨/⏳ |
| 11 | Test strategy: proptest invariants, round-trip laws, committed regressions, golden | 1 (kernel laws) + all | 🔨 |
| 3.9 | Perf budget (p95 < 16 ms; reparse < 5 ms; ~5 MB) | 2–4 (render/loop) | ⏳ |
| 4, 9.2 | `md_parse` (pulldown-cmark, byte ranges) | 2 | ⏳ |
| 9.2 | `block_tree` incremental invalidation (+ spike, oracle) | 2 | ⏳ |
| 16 | `layout`/`ColMap`; `Cursor{offset,row,desired_col}`; navigation; reveal churn | 2 | ⏳ |
| 3.2, 3.11, 13 | Live-conceal render modes; markdown construct set | 2 (model) / 3 (paint) | ⏳ |
| 3.4, 14.2 | Soft-wrap; wrap ruler; line-structure (unwrap/reflow/ventilate) | 2 (wrap) / 3 (repar) | ⏳ |
| 3.5 | `filter` primitive (argv default, caps, timeout, cancel) | 3 | ⏳ |
| 3.1, 14 | pandoc export; repar in-process transforms | 3 | ⏳ |
| 14.3 | Atomic save (`repar::atomic`); width helper | 3 | ⏳ |
| 3.8, 10 | ratatui+crossterm; sync loop; functional-core/imperative-shell | 3 (shell) / 4 (loop) | ⏳ |
| 12 | Config (TOML, precedence, project-local); keymap-as-data; command palette + menu | 4 | ⏳ |
| 5, 3.5 | Basic spellcheck (diagnostic); basic mouse | 4 | ⏳ |
| 13.2 | No-color / high-contrast accessibility | 3 (paint) / 4 (chrome) | ⏳ |
| 15 | Error handling & recovery; swap-file; panic dump | 3 (save/panic) / 4 (surface) | ⏳ |
| 5 | Incremental search (`regex-cursor`); writing aids (word count, focus) | 4 | ⏳ |

## Reuse posture (decided 2026-06-22)
"Source as much as possible" resolves, for the edit kernel, as: **reuse the hard
parts via standalone crates** (`ropey` — MIT/Apache, independent of helix-core —
plus smartstring/smallvec/unicode-*), **reuse the Helix/CodeMirror designs**, and
**hand-write the ~300-line ChangeSet/selection/undo glue** because no
ropey-based, library-friendly, permissive crate provides it (helix-core: MPL,
not-for-external-use, heavy; floem_editor_core: Apache but xi-rope, conflicts with
the locked ropey + §16). See Effort-1 plan "Reuse Posture". Also recorded v1
simplifications: **linear undo** (branching §9.1 deferred); **no `ChangeSet::compose`**
(edit-list-per-revision coalescing instead).

## Per-effort task ledgers
Each plan file ends with its own task checklist; mark tasks `- [x]` as completed and
flip this ledger's **Status** when an effort's plan is fully green.
