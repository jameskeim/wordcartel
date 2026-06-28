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
| 4b | **Async substrate + crash safety** | spec `specs/2026-06-23-wordcartel-04b-async-crash-safety.md`; plans `2026-06-23-wordcartel-04b1-async-substrate.md` (4b-1) + `2026-06-23-wordcartel-04b2-crash-safety.md` (4b-2) | **4b-1:** general std::thread+mpsc job substrate (version-stamped discard, `Executor` test-injection, plugin-ready per §18.4) + unified-channel main loop (wake on job completion) + background save (saved_version/dirty model, reuses 4a `save_atomic`) + name-keyed command registry (§10.4 boundary; migrate 4a commands). **4b-2:** swap/recovery file (idle-debounce+max-cap) + panic-dump + external-mod detection + shared modal-prompt infra. **+ deferred 4a polish:** undo/redo no-op robustness; CycleRenderMode ensure_visible; Copy-on-empty guard; shell \n-only line model (ropey `default-features=false`). **No new `repar` dep** (deferred to 4c). | **4b-1 ✅ COMPLETE** (merged 9cb8066; 9 tasks subagent-driven TDD; 103 shell + 105 core + 34 oracle green, 0 warnings; opus whole-branch review READY-TO-MERGE + Codex pre-merge gate caught & fixed a shutdown worker-join terminal-hostage bug). **4b-2 ✅ COMPLETE** (merged into master; 10 tasks subagent-driven TDD; 136 shell + 105 core + 34 oracle green, 3x-parallel stable, 0 warnings; opus whole-branch review caught & fixed an invisible-modal silent-UI bug + save&quit timing; Codex pre-merge gate caught & fixed 2 never-lose-work blockers — orphan scratch-swap recovery across restarts + failed-write phantom-checkpoint). **Effort 4b fully complete.** |
| 4r | **Buffer extraction (prep refactor)** | spec `specs/2026-06-24-multi-buffer-workspace-design.md` §6.1 | behavior-preserving substrate for multi-buffer: `Editor` → thin workspace over `Vec<Buffer>` (vec-of-one), stable `BufferId` + `next_buffer_id`/`alloc_id`, relocate per-document transient state (+ `pending_recovery`) off the flat `Editor` into `Buffer`; thread `buffer_id` through the job model (mechanical routing + debug assertion; buffer-local vs durability result classes). **No behavior change — existing suite is the gate.** Lands **before 4c** so 4c/5 build on `Buffer`. | ✅ COMPLETE (merged cf58ebb; 2 tasks subagent-driven TDD; 142 shell + 105 core + 34 oracle green, 3x-parallel stable, 0 warnings; opus whole-branch ready-to-merge + Codex pre-merge gate caught & fixed a Recover orphan-swap-cleanup regression; pre-flight also fixed a state-dir-pollution flaky test on master). 4c/5 now build on `Buffer`. |
| 4c | IO platform layer | spec `specs/2026-06-24-wordcartel-04c1-filter-export-design.md` (4c-1); plan `2026-06-24-wordcartel-04c1-filter-export.md` (4c-1) | **Decomposed (2026-06-24) into 4c-1/4c-2/4c-3.** **4c-1:** filter primitive (§3.5: argv default, shell opt-in, UTF-8 validate, size cap, timeout, Esc-cancel, deadlock-safe pipes via `subprocess` crate on a dedicated thread → `Msg::FilterDone` → by_id_mut merge w/ version-discard) + a minimal filter-command minibuffer + pandoc export presets. **4c-2:** `repar` dep + in-process transforms (Reflow/Unwrap/Ventilate, §14.1) registered as presets. **4c-3:** system-clipboard sync (`arboard`/OSC 52, §15.6). **Plugin substrate (§18.4): filter/transform commands register through 4b's registry.** **Builds on 4r's `Buffer`.** | **4c-1 ✅ COMPLETE** (merged 8ad372a; 5 tasks subagent-driven TDD; 168 shell + 105 core + 34 oracle green, 0 warnings; opus whole-branch READY + Codex pre-merge gate caught & fixed 2 real bugs — a chatty-stderr `child.wait()` DEADLOCK from stdout-only size accounting, and an export-overwrite TOCTOU). **4c-2 ✅ COMPLETE** (merged 4792c72; 4 tasks subagent-driven TDD; 185 shell + 105 core + 34 oracle + 4 integ/render green, async 3× stable, 0 warnings; `repar` path-dep behind `run_transform`; markdown-structural block-tree snapping; Ctrl+T modal chooser; sync <1MiB / async version-discard ≥1MiB merge; spec Codex-reviewed 1c+3i+2m, plan Codex-reviewed 2c+3i+1m, opus + Codex pre-merge gate both MERGE-READY). **4c-3 ✅ COMPLETE** (merged 966a1f5; 4 tasks subagent-driven TDD; 206 shell + 106 core + 34 oracle + 4 integ/render green, 0 warnings; OS-clipboard sync around the internal register — `arboard` (default-features=false, `wayland-data-control` → native Wayland + X11/Xwayland fallback) on a detached off-startup worker + OSC 52 (ST, via terminal backend writer) for SSH; async buffer-targeted Ctrl+V + register fallback + bracketed paste per UI mode; responsiveness invariants held (unbounded channel, off-startup init, OSC-via-backend-writer); spec Codex-reviewed 1c+6i+3m, plan Codex-reviewed 5i+2m, opus MERGE-READY + Codex pre-merge gate caught & fixed an empty-bracketed-paste data-loss bug). **EFFORT 4c FULLY COMPLETE (4c-1/4c-2/4c-3 all merged).** |
| 5 | App | _(later)_ | data-driven keymap/config, command palette + hideable menu, spellcheck, mouse, word/page nav, wrap-guide. **Plugin substrate (§18.4): command dispatch MUST be a name-keyed registry (key→ID→handler); keymap + palette resolve through it; thin event-hook dispatch seam. Built-ins register the same way future plugins will.** | **DECOMPOSED (2026-06-24) into 5a–5g** (each own spec→plan→build cycle). **5a** config (TOML, built-in<XDG<project-local, `--config`/`--no-config`) + data-driven keymap-over-registry + path-keyed state (resume-at-position, marks store) — FOUNDATION. **5b** command palette + hideable menu as views over the registry (§12.2). **5c** word/page nav + mouse (click/scroll/drag-select) + structural text objects (select sentence/paragraph/section) + marks — deps `unicode-segmentation` + repar segmenter + block tree. **5d** focus & authoring experience: typewriter scrolling + sentence-focus spotlight (repar sentence segmenter) + word-count goals + wrap-guide ruler (deferred here from 4c-2) + optional readability score — signature sub-effort. **5e** search **&replace** (regex) — dep `regex-cursor` (Helix-validated rope-incremental). **5f** **Harper diagnostics** (`harper-core`, Apache-2.0, in-process, offline, markdown-aware via pulldown-cmark): spell **+ grammar/style** lint via §10.4 diagnostic-Effect channel — replaces subprocess-spellcheck (caveats: English-only; `Span<char>`→byte mapping; pin 2.x). **5g** outline + jump-to-heading + fold-by-heading (block-tree-driven). DEFERRED to backlog: thesaurus (stale crate, heavy data, WordNet license), in-editor snapshots, smart-quotes (auto — violates source-as-is), templates/snippets (plugin-era). Crate due-diligence done 2026-06-24 (web-verified). | **5a ✅ COMPLETE** (merged 1399e40; 6 tasks subagent-driven TDD; 232 shell + 106 core + 34 oracle + 4 integ/render green, 0 warnings; spec `specs/2026-06-24-wordcartel-05a-config-keymap-design.md` + plan `plans/2026-06-24-wordcartel-05a-config-keymap.md`; layered TOML config (per-field merge, ordered keymap patches) + CLI parser; data-driven KeyTrie multi-key sequences over the registry (`Borrow<str>`+`resolve_name`), cua default + opt-in wordstar preset, pinned Esc precedence, retired hardcoded `key_to_command_id`; path-keyed session state (resume-at-position w/ mtime+size staleness guard, marks store, LRU prune, atomic write, saved_version-watch persist); deps toml+serde; spec Codex-reviewed 2c+6i+3m, plan Codex-reviewed 4c+2i, opus MERGE-READY + Codex pre-merge gate caught & fixed 2 bugs — a silently-dead reserved-Esc config binding + a session-LRU seq-reset eviction). **5b–5g ✅ ALL COMPLETE** (merged to master, each its own spec→plan→subagent-build→opus+Codex-gate cycle): **5b** command palette + hideable menu; **5c** word/page nav + mouse (click/scroll/drag) + structural text objects (word/sentence/paragraph) + marks + jumplist; **5d** typewriter + focus spotlight + word-count + wrap-guide + measure; **5e** regex search & replace; **5f** Harper spell + grammar diagnostics (quick-fix, diag nav); **5g** outline + jump-to-heading + fold-by-heading. All present in the command registry. **Effort 5 fully complete.** |
| 6 | **Multi-buffer workspace** | spec `specs/2026-06-24-multi-buffer-workspace-design.md` §6.2 | N open buffers, one visible/switchable: CLI multi-file, open/close/switch, close-with-dirty + multi-dirty-quit modals, per-buffer crash safety (scratch-swap re-keying, multi-buffer recovery-on-open, close-vs-in-flight durability protocol), job-routing-by-`BufferId` made real, palette switcher. **Splits-later** (window-tree over buffers, designed-for not built). | **PLANNED (pre-1.0) · exec #5** — generalizes Effort 7's open (current-buffer → N buffers; 7 builds it buffer-extensibly so this is generalization, not rework). Needs 4r + 4b (✓). ~8–9 tasks. |
| T | **Markdown color theming** | spec `specs/2026-06-26-wordcartel-theming-design.md`; plans `2026-06-27-wordcartel-theming-{01,02,03}-*.md` | (①) theme model — Color/Face/Theme/SemanticElement, Depth/quantize, 13 builtins (incl. phosphor + HSL ramp), `compose` seam, full render centralization. (②) cursor-safe prefix geometry (`ColMap.prefix_width`) + comment/byte-0-front-matter producers + blockquote/thematic/heading glyphs + selection painting + §13.2 a11y proof + front-matter incremental perf tightening. (③) base16/24 import (no YAML) + `[theme]` config (name/file/depth/styles, discriminated source, ~/relative) + env depth detection (NO_COLOR>explicit>detection) + `Depth::None` color-strip + `resolve_theme` + theme-picker overlay (live preview + relayout) + source-mode base canvas. | ✅ COMPLETE (merged `be7c853` 2026-06-27; 3 plans subagent-driven TDD; 444 shell + 176 core + 42 oracle green; 3 opus whole-branch + 4 Codex plan reviews + a Codex plan-③ code review + a full-branch Codex review **and** re-review (GO). User-visible: Tokyo Night / any base16 file loadable, `NO_COLOR`→cue mode, phosphor tints chrome **and** source text, `theme` command opens a live-preview picker. Every gate caught real bugs — named-color regression, FM incremental divergence, truncated-false-close, `Depth::None` color leak, vacuous canvas tests, ClipboardPaste-under-overlay race, focus-dim cue erasure, selection/search order.) |
| 7 | **File open / browse + Save-As** | _(to spec)_ | In-app **Open** command + a **file-browser / picker overlay** (mirrors the command-palette + theme-picker pattern: filtered, arrow-navigable list; type-to-filter path; optional directory navigation) loading via `file::open` into a `Buffer`. **Save-As** — the long-deferred "Effort 5" stub (`save.rs:90` *"No file name (save-as is Effort 5)"*): prompt for a path, atomic write, re-key swap/recovery + session state to the new path. **New (blank) document** — start an unnamed buffer in-app (first save routes through Save-As). Optional **recent files**. Reuses the overlay/prompt infra from theming + 5b; pairs with `tui-popup` (backlog) for dialogs. | **PLANNED (pre-1.0) · exec #3** — added 2026-06-27 at user request. **Save-As was orphaned** when Effort 5 decomposed into 5a–5g (no sub-effort took it); the editor currently cannot open a second file or save to a new name from within the app (files load only via the launch CLI arg). The biggest *completeness* lever. **Build the open path BUFFER-EXTENSIBLY** (open-into-current-Buffer-with-dirty-prompt now) so **Effort 6** generalizes it to N buffers without rework; the file-browser overlay is fully reusable by 6. |
| 8 | **Editor essentials** | _(to spec)_ | Small, universal commands missing today (audit 2026-06-27): **Select All** (no `select_all` command exists — Ctrl+A is bound to *move-line-start*); **Go to line** (jump to a line number via a minibuffer prompt); **cursor-position indicator** — show `Ln X, Col Y` in the status bar (currently shows path · dirty · mode · word-count, but not the caret position). All register through the §10.4 command registry; go-to-line reuses the minibuffer; the indicator is a status-line segment alongside `word_count_segment`. **Candidate follow-ons (Tier 2/3, not committed):** revert/reload-from-disk, Tab→indent / indent-dedent-selection, line ops (delete/duplicate/move/join line), help/keybinding cheatsheet. | **PLANNED (pre-1.0) · exec #1 (do FIRST)** — added 2026-06-27 at user request. Trivial, high-value gaps surfaced by a basic-editor feature audit; Select All + the Ln:Col indicator are the smallest wins. Zero dependencies — the best impact-per-cost on the board. |
| 9B | **WordStar keymap fidelity** | _(to spec; small)_ | Refs: sfwriter.com/wordstar.htm (Sawyer), wordtsar.ca/using-wordtsar. Mostly a BINDING exercise over commands that ALREADY exist: wire the **`^Q` quick-move family** (`^QR/QC` doc, `^QS/QD` line, `^QE/QX` screen → existing `move_doc_*`/`move_line_*`/`move_page_*`), `^QF` find / `^QA` replace; **fix the `^A`/`^F` preset bug** (currently bound to *char* `move_left/right`; should be `move_word_left/right` — a shipped bug, fixable in ~1 line and landable immediately, independent of the rest); complete the `^K` prefix; **numbered bookmarks** `^K0–9` set / `^Q0–9` jump (char-keyed marks already exist). **OUT of scope (design boundary — keep markdown source-as-is):** `^P` print-control codes, `^O` on-screen-format (margins/justification/tabs/ruler), dot commands, worksheet/dBASE import, mail-merge — wordcartel's answer is **markdown syntax + theming + pandoc export** (Effort T / 4c), not embedded format codes. | **PLANNED (pre-1.0) · exec #2 (quick win).** Near-free; serves the WordStar-writer direction. The `^A`/`^F` fix can land NOW, standalone. |
| 9A | **Persistent marked blocks** (WordStar "mark now, act later") | _(to spec; brainstorm-first)_ | The headline new primitive: a **decoupled, range-valued, edit-tracking** region — block-start (`^KB`) / block-end (`^KK`) — SEPARATE from the live CUA selection: it **survives cursor movement and edits** (its two endpoints follow the text via the same `map_pos`-on-edit machinery that already moves point-marks, editor.rs:95), is rendered with a **distinct highlight/cue** (its own §13.2-safe face, not the Selection face), and is **acted on later from anywhere** — the SOURCE is the persistent block, the TARGET is wherever the caret is now: copy-block→cursor (`^KC`), move-block→cursor (`^KV`), delete-block (`^KY`), write-block→file (`^KW`, **needs Effort 7's Save-As**), hide/show (`^KH`); jump-to-block (`^QB`/`^QK`). The one WordStar idea that is a true new primitive, not a rebinding — Sawyer's "mark a block, then edit for an hour before doing anything with it…you're in control, not the program." Builds on: edit-tracking marks (point→range), selection/clipboard, the keymap registry, the §13.2 cue model. | **PLANNED (pre-1.0) · exec #4** — after Effort 7 (for `^KW`). The signature differentiator; **brainstorm-first** (one block vs several; interaction with the live selection + the clipboard; `^KW`↔Save-As). |
| P | **Plugin System (in-process Lua)** | _(post-1.0)_ | mlua runtime; sandboxed editor API (read state, edit via the single `apply` channel, register commands, bind keys, event hooks, jobs); security/permission model; plugin config + `--no-plugins` | **PLANNED (pre-1.0 capstone) · exec #6 (LAST)** — depends on the full feature set; the substrate was deliberately built into Efforts 5 + 4b (§18). Largest remaining effort. |

## Execution sequence (remaining — all pre-1.0)

**Decided 2026-06-27: 1.0 = the *entire* remaining roadmap — no post-1.0 deferral.**
Everything below ships before 1.0. Listed by **execution order** (not by historical
ID number). Cheapest-and-highest-value first; nothing is built that a later step
discards. (Done already: Efforts 1–5 incl. 5a–5g, and **T — theming** ✅.)

| Order | Effort (ID) | Why here | Cost |
|---|---|---|---|
| **0** | `^A`/`^F` word-motion fix *(part of 9B)* | shipped `wordstar`-preset bug; ~1 line; landable anytime, standalone | XS |
| **1** | **Editor essentials (8)** | universal daily value (Select All, Go-to-line, Ln:Col), zero deps, instant polish | XS |
| **2** | **WordStar keymap fidelity (9B)** | near-free — bindings over commands that already exist; serves the WordStar direction | XS–S |
| **3** | **Open / Save-As / New + browser (7)** | the *completeness* lever; **build the open path buffer-extensibly** for #5 | M |
| **4** | **Persistent marked blocks (9A)** | signature differentiator; needs 7's `^KW`; **brainstorm-first** | M |
| **5** | **Multi-buffer workspace (6)** | generalizes 7's open (current-buffer → N buffers) | L |
| **6** | **Plugin system — Lua (P)** | capstone; depends on the full feature set; substrate already in place | L |

**Dependency spine:** 8 and 9B are independent (can even run in parallel); **7 → 9A**
(the `^KW` write-block-to-file needs Save-As); **7 → 6** (multi-buffer generalizes the
open path); everything → P. The single cross-effort seam to design for is **7's open
path** — written buffer-extensibly so 6 *generalizes* it rather than rewrites it.

**Why this flows:** every step ships visible value cheapest-first; it front-loads the
two changes that most make wordcartel "a real editor you live in" (8 then 7), places the
WordStar work early-and-cheap (9B) then deep-and-differentiating (9A), and ends with the
two largest, most-dependent efforts (6 then P) once the feature set they build on is stable.

> Effort 4 (IO/Shell, §10/§3.8/§14/§15) split into **4a** (the synchronous runnable
> editor — open/render/edit/navigate/save), **4b** (async job substrate + crash
> safety: background save, swap/recovery, panic dump, external-mod, command
> registry), and **4c** (the IO *platform* layer: filter primitive, repar
> transforms, pandoc export, system-clipboard sync). 4a ships a usable editor; 4b
> makes "never lose work" real and moves slow IO off the keystroke path; 4c adds the
> Unix-pipe power features. The 4b/4c split was made when 4b was specced
> (2026-06-23) to keep the safety-critical substrate separable from the platform
> features. Data-driven keymap + palette + menu remain Effort 5.

> Effort 2 (Render Kernel, §16/§13/§9.2) split into Plan 2 (render core — inline
> conceal/style + the spike-validated layout/ColMap/cursor) and Plan 3 (incremental
> block_tree + block-role rendering). Seam: md_parse/layout take a line's **block
> role as input**; block_tree computes roles in Plan 3. Layout ports the validated
> spike at `~/projects/wordcartel-layout-spike`.

### Effort 5 — candidate dependencies (to evaluate when the plan is written)

Surveyed 2026-06-23. These are ratatui-ecosystem widgets that map onto the
Effort 5 surfaces (command palette, hideable menu, dialogs). **Not committed** —
verify current version + maintenance status at adoption time. The hard
architectural guardrail (§12.2): the palette/menu are **view layers over the
name-keyed command registry** (the plugin substrate) — a widget renders and
routes through the registry, it never owns the command list.

| Crate | Maps to | Verdict | Notes |
|---|---|---|---|
| [`tui-popup`](https://github.com/joshka/tui-popup) | dirty-quit confirm (replaces `pending_quit` ad-hoc flag), save-as / error dialogs | **Adopt candidate** | By a core ratatui maintainer (joshka); low-risk centered popup. |
| [`tui-menu`](https://github.com/shuoli84/tui-menu) | §12.2 hideable menu bar (File/Edit/Format/Insert/View/Export) | **Evaluate** | Tree/dropdown widget; treat as the view layer fed by the command registry, and confirm it can display each entry's chord (§12.2). Brings its own selection state. |
| [`tui-overlay`](https://crates.io/crates/tui-overlay) | generic overlay compositing (palette over menu, stacked dialogs) | **Fallback** | Overlaps `tui-popup`. Pick ONE overlay strategy; reach for this only if popup can't stack. |
| [`ratatui-image`](https://crates.io/crates/ratatui-image) | inline image display (kitty/iTerm2/sixel + capability detection) | **Backlog only** | §13.3/§13.5 defer inline images; v1 path is "open image externally". The crate to use *if* images get promoted off the backlog — not a 1.0 dep. |
| [`tui-shimmer`](https://github.com/vinhnx/tui-shimmer) | decorative animated text (splash/empty-state) | **Out of scope (1.0)** | Tension with §3.2 distraction-free + §3.9 responsiveness (repaint churn). Opt-in splash at most, never in the editing surface. |
| [`tui-tabs`](https://crates.io/crates/tui-tabs) / [`ratatui-comfy-tabs`](https://crates.io/crates/ratatui-comfy-tabs) | tab bar | **Out of scope (1.0)** | v1 is single-document/single-pane; tabs imply an undesigned workspace model that cuts against minimal chrome. Reassess only if multi-document editing becomes a post-1.0 goal. |

The palette's fuzzy-search stack is already chosen in-spec (`nucleo`, §12.2); the
menu/popup widget is the open choice this table tracks.

## Spec → effort map

Legend: ✅ done · 🔨 in this effort · ⏳ later effort · 📋 deferred to impl-spec detail.

| Spec § | Item | Effort | Status |
|---|---|---|---|
| 3.1 | Markdown-as-source-of-truth (`.md`, text not AST) | 1 (buffer holds text) / 4a (save) | ✅ |
| 3.3 | Text not AST; plain-text edits | 1 | ✅ |
| 3.10, 16.1 | `ropey` buffer; **byte offset = canonical position** | 1 | ✅ |
| 9.1 | Undo = ChangeSet (retain/delete/insert) + branching history; `smartstring`; prose-tuned coalescing (~500 ms; break on paste / programmatic / cursor-move) | 1 | ✅ |
| 9.1, 3.6 | Selection = anchor/head over byte offsets; `SmallVec<[Range;1]>`; `.map(&ChangeSet)` | 1 | ✅ |
| 10.1 | Single mutation channel `apply(Transaction)`; selection mapped on the same atomic step | 1 (kernel `apply`) / 4a (wired loop) / 4b (job `merge` routes doc edits through `apply`) | 🔨/⏳ |
| 10.2 | `version: u64` revision token | 1 | 🔨 |
| 10.3 | O(1) rope snapshots for async workers; reconcile = version-discard | 1 (snapshot API) / 4b (`2026-06-23-wordcartel-04b-async-crash-safety.md`: job substrate + workers) | 🔨/⏳ |
| 3.6, 9.5, 15.6 | In-process clipboard **register** (system sync is effort 4c) | 1 (register) / 4a (wired) / 4c (system sync: `arboard`/OSC 52) | 🔨/⏳ |
| 11 | Test strategy: proptest invariants, round-trip laws, committed regressions, golden | 1 (kernel laws) + all | 🔨 |
| 3.9 | Perf budget (p95 < 16 ms; reparse < 5 ms; ~5 MB) | 2–4 (render/loop) | ⏳ |
| 4, 9.2 | `md_parse` (pulldown-cmark, byte ranges) | 2 | ✅ (inline; images/CommonMark-exact escapes -> Plan 3) |
| 9.2 | `block_tree` incremental invalidation (+ spike, oracle) | 3a | ✅ (merged; oracle: single/multi-edit × ASCII/multibyte + delete-to-empty; 4 latent bugs caught & fixed) |
| 16 | `layout`/`ColMap`; `Cursor{offset,row,desired_col}`; navigation; reveal churn | 2 | ✅ (layout/ColMap/cursor done; reveal-churn scroll-anchor -> app) |
| 3.2, 3.11, 13 | Live-conceal render modes; markdown construct set | 2 (inline) / 3b (block-prefix conceal) / 4a (paint) | ✅ conceal model (inline §2 + block-prefix/role §3b incl. ATX tabs/empty/closing, list→bullet, quote, fence, setext, thematic break); terminal paint shipped in 4a |
| 3.4, 14.2 | Soft-wrap; wrap ruler; line-structure (unwrap/reflow/ventilate) | 2 (wrap) / 4c (repar transforms) / 5 (wrap ruler) | ⏳ |
| 3.5 | `filter` primitive (argv default, caps, timeout, cancel) | 4c | ⏳ |
| 3.1, 14 | pandoc export; repar in-process transforms | 4c | ⏳ |
| 14.3 | Atomic save (4a hand-rolled `save_atomic`, no `repar` dep) | 4a | ✅ (merged d94fc39: same-dir O_EXCL temp, fsync, rename, dir-fsync, symlink refusal, mode preserve, skip-unchanged) |
| 3.8, 10 | ratatui+crossterm; sync loop; functional-core/imperative-shell | 4a (sync shell+loop) / 4b (unified-channel loop, wake on job completion) | 🔨/⏳ |
| 10.4 | **name-keyed command registry (key→ID→handler; plugin substrate)** | 4b (`2026-06-23-wordcartel-04b-async-crash-safety.md` §4.4: mechanism + migrate 4a commands) | 🔨 |
| 12 | Config (TOML, precedence, project-local); keymap-as-data; command palette + menu (resolve through the 4b registry) | 5 | ⏳ |
| 5, 3.5 | Basic spellcheck (diagnostic); basic mouse | 5 | ⏳ |
| 13.2 | No-color / high-contrast accessibility | 4a (paint) / 5 (chrome) | ⏳ |
| 15 | Error handling & recovery; swap-file; panic dump; external-mod detection; background-save failure keeps file+dirty | 4a (atomic save) / 4b-1 (bg save) / 4b-2 (swap/recovery + orphan scratch recovery, panic dump, external-mod modal, modal surface) | ✅ (merged; swap idle+max-cap cadence, version-aware lifecycle, hash-first recovery, try_lock panic dump; remaining: 3-way merge backlog) |
| 5 | Incremental search (`regex-cursor`); writing aids (word count, focus) | 5 | ⏳ |
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
