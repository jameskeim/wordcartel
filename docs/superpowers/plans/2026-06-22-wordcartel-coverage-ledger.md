# Wordcartel — Implementation Coverage Ledger

**Purpose:** trace every spec requirement to the effort (plan) that implements it,
so nothing falls through the cracks. Update the **Status** as efforts complete.

Spec: `docs/superpowers/specs/2026-06-21-wordcartel-design.md`

## Efforts (plans)

| # | Effort | Plan file | Produces | Status |
|---|---|---|---|---|
| 1 | **Edit Kernel** | `2026-06-22-wordcartel-01-edit-kernel.md` | pure buffer + ChangeSet undo + selection (headless lib) | ✅ COMPLETE (branch `effort-1-edit-kernel`, 28 tests green, final review READY-TO-MERGE + I-1/I-2/I-3 hardening) |
| 2 | **Render Core** | `2026-06-22-wordcartel-02-render-core.md` | md_parse (inline conceal+style) + layout/ColMap/cursor-nav (port spike) | ✅ COMPLETE (merged a40f465; 58 tests incl 6 laws@512; Codex gate caught 2 cursor bugs) |
| 3a | **block_tree** | `2026-06-22-wordcartel-03a-block-tree.md` | full_parse + incremental_update (spike-validated; oracle gate) | ✅ COMPLETE (merged 1beb09b; 90 tests; strengthened oracle found+fixed 4 real bugs; Codex MERGE-READY) |
| 3b | **block-role rendering** | `2026-06-22-wordcartel-03b-block-roles.md` | BlockKind heading-level + role_at query + md_parse block-prefix conceal + VisualRow role/glyph | ✅ COMPLETE (merged 4e88368; 95 lib + 27 oracle green; final opus review + Codex gate; oracle found+fixed 2 pre-existing 3a container-merge bugs; Codex found+fixed 3 conceal-fidelity gaps) |
| 3c | **block_tree rope integration** | `2026-06-23-wordcartel-03c-blocktree-rope.md` | TextSource trait (&str + &Rope); incremental reparse materializes only the edited region (O(region)) so shell derive is O(visible)+O(edited), §3.9; oracle extended to str==rope==full | ✅ COMPLETE (merged 9f3f38d; 105 lib + 34 oracle green; opus READY-TO-MERGE + Codex pre-merge gate fixed an O(doc) gap-scan; perf probe: 78KB doc → 81-byte reparse; LF-only rope scan, str==rope==full @15k cases) |
| 4a | **Terminal shell (sync)** | `2026-06-23-wordcartel-04a-terminal-shell.md` | runnable `wcartel` editor: Editor/Document/View + apply + derive (O(region) rope-incremental) + ratatui live-preview render + crossterm loop + cursor nav (cross-line/wrapped/intra-line-scroll) + edit/select(+replace)/clipboard/undo + atomic save | ✅ COMPLETE (merged d94fc39; 84 shell + 105 core + 34 oracle green; Codex plan-review + opus whole-branch + Codex pre-merge gates each found+fixed real bugs: selection-replace on type/paste/delete, intra-line scroll for tall paragraphs, dir-fsync durability, raw-mode rollback, loop double-rebuild). **Deferred to 4b:** undo/redo no-op robustness (dirty on empty-history undo; desired_col reset); CycleRenderMode ensure_visible; Copy-on-empty stores ""; shell len_lines unicode-vs-\n on bare \r/U+2028. |
| 4b | IO async edges | _(later)_ | std::thread+mpsc worker (version-stamped discard), background save + swap/recovery + panic dump, filter primitive, repar transforms, system-clipboard sync, external-mod detection, targeted undo/redo reparse. **+ deferred 4a polish:** undo/redo no-op robustness (no spurious dirty; reset desired_col); CycleRenderMode ensure_visible; Copy-on-empty guard; shell \n-only line model (vs ropey unicode len_lines). **Plugin substrate (§18.4): keep the job/filter API general enough to host a plugin-invoked transform.** | PLANNED |
| 5 | App | _(later)_ | data-driven keymap/config, command palette + hideable menu, spellcheck, mouse, word/page nav, wrap-guide. **Plugin substrate (§18.4): command dispatch MUST be a name-keyed registry (key→ID→handler); keymap + palette resolve through it; thin event-hook dispatch seam. Built-ins register the same way future plugins will.** | PLANNED |
| P | **Plugin System (in-process Lua)** | _(post-1.0)_ | mlua runtime; sandboxed editor API (read state, edit via the single `apply` channel, register commands, bind keys, event hooks, jobs); security/permission model; plugin config + `--no-plugins` | PLANNED — **post-1.0** (§18); substrate built into Efforts 5 + 4b |

> Effort 4 (IO/Shell, §10/§3.8/§14/§15) split into **Plan 4a** (the synchronous
> runnable editor — open/render/edit/navigate/save) and **Plan 4b** (async edges,
> slow IO, filters, repar, crash recovery). 4a ships a usable editor; 4b hardens
> the edges & adds power features. Rationale in the 4a plan's Reuse Posture /
> Self-Review. Data-driven keymap + palette + menu remain Effort 5.

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
| 4, 9.2 | `md_parse` (pulldown-cmark, byte ranges) | 2 | ✅ (inline; images/CommonMark-exact escapes -> Plan 3) |
| 9.2 | `block_tree` incremental invalidation (+ spike, oracle) | 3a | ✅ (merged; oracle: single/multi-edit × ASCII/multibyte + delete-to-empty; 4 latent bugs caught & fixed) |
| 16 | `layout`/`ColMap`; `Cursor{offset,row,desired_col}`; navigation; reveal churn | 2 | ✅ (layout/ColMap/cursor done; reveal-churn scroll-anchor -> app) |
| 3.2, 3.11, 13 | Live-conceal render modes; markdown construct set | 2 (inline) / 3b (block-prefix conceal) / 4 (paint) | ✅ conceal model (inline §2 + block-prefix/role §3b incl. ATX tabs/empty/closing, list→bullet, quote, fence, setext, thematic break); terminal paint → 4 |
| 3.4, 14.2 | Soft-wrap; wrap ruler; line-structure (unwrap/reflow/ventilate) | 2 (wrap) / 3 (repar) | ⏳ |
| 3.5 | `filter` primitive (argv default, caps, timeout, cancel) | 3 | ⏳ |
| 3.1, 14 | pandoc export; repar in-process transforms | 3 | ⏳ |
| 14.3 | Atomic save (`repar::atomic`); width helper | 3 | ⏳ |
| 3.8, 10 | ratatui+crossterm; sync loop; functional-core/imperative-shell | 3 (shell) / 4 (loop) | ⏳ |
| 12, 10.4 | Config (TOML, precedence, project-local); keymap-as-data; command palette + menu; **name-keyed command registry (plugin substrate)** | 5 | ⏳ |
| 5, 3.5 | Basic spellcheck (diagnostic); basic mouse | 4 | ⏳ |
| 13.2 | No-color / high-contrast accessibility | 3 (paint) / 4 (chrome) | ⏳ |
| 15 | Error handling & recovery; swap-file; panic dump | 3 (save/panic) / 4 (surface) | ⏳ |
| 5 | Incremental search (`regex-cursor`); writing aids (word count, focus) | 4 | ⏳ |
| 18 | Plugin system (in-process Lua): registry/hook substrate; sandboxed `apply`-channel API; security; config | P (post-1.0); substrate in 5/4b | 📋 |

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
