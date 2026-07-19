# Feature / UX backlog — OPEN item triage

**How this file works (backlog manifest system, since 2026-07-10):** this document holds the **triage
prose for OPEN feature/UX items only** — grounded facts, design directions, and forks — each keyed to
the manifest by a `<!-- item: ID -->` marker beneath its heading. **Status, size, kind, and
shipped/dropped state live ONLY in `backlog.toml` (repo root); do NOT record status here.** To see what
is open vs done, read the generated **`BACKLOG.md`** (repo root) — never scan this prose. Completed /
dropped items' prose lives in [`backlog-archive.md`](backlog-archive.md); structural /
engineering-health items live in [`engineering-health.md`](engineering-health.md). Add or edit an item
with `scripts/backlog add` (or a `bl:` message) then `scripts/backlog bless`; a `cargo test` gate
(`wordcartel/tests/backlog.rs`) keeps the manifest, markers, and dashboard in sync. Design doc:
`docs/superpowers/specs/2026-07-10-backlog-tracking-system-design.md`.

Each open item graduates to the standard gated pipeline (brainstorm → spec → Codex/Fable review → plan
→ subagent build) when picked up.

---

## Governing principle — the command-surface contract

**The authoritative App law lives in `docs/design/command-surface-contract.md`** (a governing
contract, not backlog triage) and is a conformance gate on every spec + plan (`CLAUDE.md` →
Development process). The laws, in brief (each has an enforcing test — a violation is a bug):

1. **The registry is the single source of truth** (`palette.rs:66-86` iterates the whole registry;
   the pinned test at `palette.rs:138` asserts "empty query → all commands").
2. **Every user-settable option IS a command** (a persisted setting → a command changes it).
3. **The palette is exhaustive** (every non-internal command appears).
4. **The menu is a curated subset** (menu ⊆ palette; ~58 commands tagged `menu: Some(category)` in
   `CommandMeta`, `registry.rs:45-48`, tree built by `menu::grouped_commands`).
5. **Every mouse affordance has a keyboard path** (falls out of law 3).
6. **One shared setter per option; profiles call it too** (no bypass — profile can't drift from the
   command).
7. **Hints track the active keymap** (re-resolved on preset switch — A5; prefer the user's explicit
   binding over the shortest default).

Shape rules: multi-state option = set-per-state primitives (palette-only) + a cycle (menu, state-in-
label); a preset is a convenience over primitives, never the only door; commands are the Effort-P
plugin/automation spine. **The one judgment call** — menu (browse-by-category) vs palette-only
(motions, plumbing, keystroke-native, set-per-state primitives) — is applied item-by-item in **A3b**.
A3 fixes the contract *violation* the ZEN/FULL density gap opened (orphaned `status_line`/`scrollbar`
options) + locks the hint plumbing; the state-in-label display shipped with E2.

---

## Theme B — rendering fidelity

### B6. Heading-glyph STYLE toggle — shades / Nerd numerals / inverted numerals
<!-- item: B6 -->

**Idea (user):** offer the heading-level glyph as a selectable STYLE, cycling among three looks:
(1) the current shade ramp; (2) Nerd Font numeric-box glyphs `󰬺 󰬻 󰬼 󰬽 󰬾 󰬿` (U+F0B3A–F, Material-Design
"numeric-N-box"); (3) inverted numerals — a reverse-video digit `1`–`6`.

**Pinned design (from the 2026-07-09 exploration):** all three fit the CURRENT 2-cell gutter
(`glyph + space`, `prefix_width = 2`, `layout.rs:289`) — the cheap tier, no layout/caret/wrap change.
Render just picks a glyph table + whether to add a `reverse` modifier on the glyph (NOT the space):
- **Nerd** — reversed `󰬺`..`󰬿` + a normal space. **This is the CURRENT default** (shipped 24d87bb,
  `render.rs:25`). Single-width (`wcwidth=1`; but `east_asian_width=A`, ambiguous — may render 2-wide on
  wide-ambiguous terminals). Requires a Nerd Font (tofu otherwise) — so the toggle must offer a universal
  fallback.
- **Shades** — `█ ▆ ▅ ▄ ▃ ▂` (the pre-24d87bb B5 ramp), dim, no reverse. Font-universal.
- **Inverted numeral** — reversed `1`..`6` + a normal space. Font-universal.
The reversed box's fill = the heading level's fg colour, so per-level heading colours tint the box.

**Open forks (for the brainstorm):**
- On/off model: fold the existing `heading_level_glyph` bool (`theme.rs:119`) into the style enum as an
  `Off` state (one 4-way control) vs keep on/off separate from the 3-way style.
- Where it lives: a runtime user cycle command (palette/menu/keybind, persisted — command-surface tax,
  templated on `cycle_scrollbar` / `clipboard_provider`, `registry.rs:480-510`) vs a theme-only property.
- Default MUST stay **Shades** (universal); Nerd is opt-in (font dependency). The minimal themes
  (no-color / terminal-plain / terminal-ansi) should not default to Nerd.

**Difficulty:** Small–Medium, one effort, templated. Cost = the command-surface invariant-test gates +
heading golden/pin churn across three styles. Anchors: `SHADES` (`render.rs:20`), heading paint sites
(`render.rs:~665,~730`), `prefix_width` (`layout.rs:289`), `heading_level_glyph` (`theme.rs:119`),
multi-state-option template (`registry.rs:480-510`).

## Theme C — document workflow

## Theme D — configuration & persistence

## Theme E — product identity: minimalist by default, complete on demand

## Theme H — code health

## Theme R — editing responsiveness (the project's #1 invariant: instant typing)

## Theme S — manuscript structure (the "TUI corkboard")

**Origin:** 2026-07-07 design chat, prompted by the beloved-features report
(`~/projects/wordprocessing/beloved-features-report.md`). The report's biggest gap for
wordcartel vs the process-centric studios (Scrivener/Ulysses/Longform) is
**manuscript-as-rearrangeable-fragments** — the corkboard/binder. Two ways to deliver it in a
TUI, at two different zoom levels. **Key framing:** in markdown the *headings ARE the binder* —
no separate data model to build or desync (a real Scrivener failure mode). S1 and S2 are the
same verb (rearrange fragments) at intra-document vs inter-document scale; S2 can reuse S1's
list/drag interaction surface with files-as-items instead of headings-as-items.

**Prior art (checked 2026-07-07):** the core operations are proven and beloved in the terminal,
but a *prose-first, markdown-native TUI corkboard as a coherent product does not appear to
exist* — the corkboard tools (Scrivener, Manuskript) are all GUI. So we're combining proven
primitives into an unoccupied niche, not inventing risky mechanics.
- **S1 engine — Emacs org-mode Structure Editing** is the canonical prior art: `M-↑`/`M-↓` move
  a subtree (level-preserving sibling swap), `M-←`/`M-→` promote/demote — a clean precedent that
  *reorder* and *re-parent* are SEPARATE commands (answers our normalize-vs-preserve fork:
  keep sibling-reorder level-preserving; make promote/demote explicit). Emacs **markdown-mode**
  does exactly this for markdown (`C-c ↑`/`C-c ↓` = `markdown-move-up/down`, subtree moves).
  These live inside general editors, not a prose word processor — that's our differentiation.
- **S1 view surface** — `aerial.nvim` / `outline.nvim` (tree outline sidebars) and **treemd**
  (TUI dual-pane markdown outline+render viewer) prove the TUI structure-view layout, but for
  *navigation only*, not reorder. `vim-markdown-folding` proves fold-by-section (approach B).
- **S2 model** — directory + ordered manifest + compile exists as build tools (**mdBook**'s
  `SUMMARY.md` — Rust; Quarto; Bookdown; Leanpub) and as a GUI plugin (**Obsidian Longform**),
  but NOT as an interactive TUI binder. Manuskript (GUI, FOSS) is the closest Scrivener-clone
  sibling.

### S1. Rearrangeable outline / heading-subtree corkboard
<!-- item: S1 -->

**What:** promote the transient outline overlay into a dwellable "structure mode" (or an
in-place folded-reorder in the main buffer — the two surfaces of the same primitive). The
foundational operation is a **heading-subtree move**: take a heading + everything under it up to
the next heading of the *same-or-higher* level (deeper headings are part of the subtree — the
same boundary `folds` already compute via `outline::heading_starts`), cut that byte range,
reinsert elsewhere. One atomic edit through `submit_transaction`/`ChangeSet` (valid-by-
construction, single undo step, no half-apply — stays inside the no-data-loss invariant). Mouse
drag-to-reorder is now cheap (mouse completeness shipped). Reuses: block tree, `outline`, folds,
transactions, marks — all already core.

**Core/plugin: CORE (pre-Effort-P).** It's structural *editing* on the data-integrity path — a
subtree move is a valid-by-construction transaction; a bad one corrupts the manuscript
(worst-case = data loss → must be core). The move primitive + the default structure-mode view
are core; a fancier card-grid view (approach C) could later be a plugin layered on the core
command. Feasible now — the machinery exists.

**Design forks (for the brainstorm):**
1. **Primary surface:** (A) enhanced outline "structure mode" (rich, dwellable, drag-reorder)
   vs (B) in-place folded reorder in the main buffer (minimal, no mode switch) vs both.
2. **Reorder vs re-parent:** level-preserving sibling swap as the common path; promote/demote as
   a separate explicit command (org-mode precedent). On a cross-level move, `normalize-on-drop`
   (shift the whole subtree's `#`-depth by the delta, clamp at H6, skip fenced code — the block
   tree knows code spans) vs `preserve-level`.
3. **Card "synopsis":** derived (heading + first non-empty line) by default — zero storage, pure
   markdown; optional `> blockquote`-under-heading convention as an authored synopsis.
4. **Edge cases:** content before the first heading; headings-inside-code-fences; a doc with no
   headings (degrade gracefully to "no cards").

**Prior art — Fresh (`sinelaw/fresh`) windowing, read at source 2026-07-11.** Two findings bear
directly on the structure/card view:
- **Render into a `&mut Buffer`, not a `Frame`.** Fresh's split renderer paints into an arbitrary
  ratatui `Buffer`, decoupled from the live terminal draw — which is what makes offscreen previews
  (their "phantom leaf") cheap. Adopting this seam gives corkboard **card thumbnails** and golden
  render tests for free.
- **Content-agnostic leaf, not a typed pane / new `RenderMode` variant.** Fresh holds any buffer in a
  leaf and reads a small `virtual_mode()` flag at paint time; singleton panels route through one
  tagged "dock" leaf (`SplitRole::UtilityDock`), never a new node type. Lesson: model structure mode /
  a card grid as **a buffer with a mode**, not a new `RenderMode` variant + render arm — keeps the
  dispatcher closed to editing (our Module-structure GATE).

Cross-cutting render lessons from the same read live elsewhere, not here: the **`Scene` derive-once
view-model** (menu/palette/status projected once off the registry, hit-geometry from render caches)
belongs to the command-surface-contract lineage; the **two-tier visible-window caching** (FIFO
byte-budget line-wrap + prefix-sum row index) is an R-theme latency reference.

### S2. Directory-as-binder (project/manuscript over many files)
<!-- item: S2 -->

**What:** treat a *directory of `.md` files* as a manuscript — each file a scene/chapter
("card"), plus an **ordered manifest** (filesystem order ≠ manuscript order) and a **compile**
step to concatenate for export/reading. Reuses the existing multi-buffer system for
open/switch. This turns wordcartel from a *document editor* into a *project editor*.

**Core/plugin: PLUGIN (post-Effort-P) — and strategically so.** It's an opt-in project/workflow
layer that only *orchestrates* existing ops (open via multi-buffer, write via save, compile via
a transform-like step); worst case from a bug is a wrong *export*, not lost source → plugin-safe.
Three reasons: (1) prior art agrees — Obsidian Longform is a plugin; (2) identity — an opinionated
workflow shouldn't be baked into the core's single-plain-text-file minimalism; (3) it's the ideal
**first real plugin / API driver** — building it forces P's API to expose buffer/file/command/job
access. Waits for Effort P.

**Different beast from S1 (recorded so we don't conflate them):** S1 = intra-document (move
text ranges, no new data model, one file stays one file); S2 = inter-document (reorder a
manifest, needs a compile step, the "document" becomes a convention over a directory). S2
reintroduces exactly the two frictions the report flags — a structure that can desync, and a
compile step ("the most complained-about feature in any writing software"). Justified only at
book scale (isolation, per-scene git history, per-scene notes, true binder feel). **They
compose:** a book = a manifest (S2) of chapter files, each with rearrangeable scene-headings
(S1). Sequence S2 *after* S1; S2 can reuse S1's rearrange UI with files as items.

**Design forks (deferred until S1 lands / the writing-unit question is answered):** manifest
format (own file? frontmatter? mdBook-style `SUMMARY.md`?); compile semantics (heading-level
offset per file? separators?); how it coexists with single-file mode; whether it's core or a
post-Effort-P Lua plugin (a strong plugin candidate — it's a project *layer* over the editor).

**Open question for the human:** the S1-vs-S2 priority hinges on writing unit — single long
document reshaped internally (→ S1 is the whole answer) vs book-as-many-files (→ S2 on top of
S1). Not yet decided.

### S3. Snapshots — named, durable revision checkpoints ("fearless editing")
<!-- item: S3 -->

**What:** Scrivener-style snapshots — capture the document at a point in time (named/
timestamped), list them, **compare (diff)** against current, and **restore** with one action.
The report's "fearless-revision insurance." **This is the lowest-risk, highest-architecture-fit
of the three manuscript gaps** — it is essentially the user-facing surface of the existing
durability spine (feasibility checked 2026-07-07).

**Enablers already present (the expensive parts):**
- **O(1) content capture** — `TextBuffer::snapshot() -> ropey::Rope` (`buffer.rs:99`; ropey is
  copy-on-write, so N snapshots of a lightly-edited doc share memory). Already used live:
  `recovery.rs:8` keeps `LAST_GOOD: Mutex<Option<(path, Rope)>>` as a retained point-in-time
  snapshot for crash recovery — the exact pattern in production.
- **Safe restore** — `change.rs` (*"ChangeSet: reversible byte-diff"*) + `history.rs`
  (`History { revisions }`, apply/undo/redo, M5 budget eviction). Restore = one **replace-all**
  ChangeSet through the transaction path → atomic, single undo step, no half-apply (inside the
  no-data-loss invariant). Restore does NOT need a display diff.
- **Durable persistence** — `save_atomic`/`save_atomic_bytes` (`file.rs`) over the M3 `Fs` seam;
  snapshots can be plain `.md` files in a sidecar dir (keeps "file over app" — you can `cat`
  your history).
- **Dedup / labels** — `swap.rs` FNV-1a `content_hash` + `version` (skip identical snapshots;
  timestamp/version labels).

**The one genuine net-new algorithm:** a **display diff** (line/word compare for the "what
changed vs this snapshot" view). None exists today — the settings *diff-law* and the ChangeSet
*reversible byte-diff* are both unrelated (a settings merge and a transaction, not a text
compare-for-display). Pragmatic: add the `similar` crate, or a small Myers impl. Pure-core,
well-understood; the ONLY new capability. (First cut could ship capture+list+restore WITHOUT
the diff and add the compare view second.)

**Also net-new (additive):** a snapshot store (per-buffer `Vec<Snapshot { rope, taken_at,
label, version, hash }>` + on-disk format) and a snapshots overlay + commands (take / list /
preview / diff / restore) reusing the overlay + `list_window` + mouse + palette/menu framework.

**Design forks:** snapshot granularity (whole buffer vs per-heading-subtree — composes with
S1); retention policy (keep all / cap N / user-prune — undo already has M5 budget eviction as a
precedent); on-disk format + location; whether the diff view is line- or word-level.

**Distinction to keep explicit:** Snapshots ≠ undo. Undo (`history.rs`) is fine-grained,
automatic, ephemeral, in-session, budget-evicted. Snapshots are coarse, deliberate, named,
durable across sessions. Different layer; a restore lands as one undoable revision but snapshots
neither replace nor depend on the undo stack.

**Core/plugin: CORE (pre-Effort-P), policy tunable.** Restore is a data-mutating transaction and
persistence uses the atomic writer — both are integrity/durability territory (worst-case = losing
current work), and the feature's *whole value is data safety*, so it belongs to the layer that owns
"no data loss." The safety-critical spine (snapshot store, restore-transaction, snapshot-write) is
core; *policy* (auto-snapshot triggers, retention count, line-vs-word diff) can be config- or
plugin-tunable via hooks. Feasible now.

## Theme P — plugin candidates from the beloved-features report

**Origin:** 2026-07-07, same design chat as Theme S. A pass over the whole report
(`~/projects/wordprocessing/beloved-features-report.md`) for *unimplemented* beloved features
that are better delivered as opt-in Lua plugins (Effort P) than baked into core. **Key
principle:** several of these are features the report is openly *ambivalent* about (goals "bleed
the joy," Hemingway-fails-Hemingway, AI's "uninvited co-author") — making them opt-in plugins is
the *correct* resolution of the minimalism-vs-features tension, not a compromise. Boundary test:
off-hot-path + worst-case-is-wrong-output-not-lost-data + prescribes-a-workflow → plugin. **All
items here are POST-Effort-P** (they need the plugin API). None is committed scope; this is the
durable candidate list + a de-facto requirements probe for the P API.

### P-A. Analysis / policy plugins — cleanest fit, high infra reuse
<!-- item: PA -->

- **Writing goals / targets / streaks** — motivation layer computed on save/idle; opt-in matches
  the "goals bleed the joy" counter-literature. Reuses word count + status line + a sidecar file.
- **Readability / style lens** (Hemingway-style: long sentences, adverbs, passive) — an analysis
  *job* whose findings surface as dismissible marks; opt-in matches the anti-prescriptivist
  evidence (Hemingway rated "Bad"). **Highest infra reuse:** the diagnostics + quick-fix overlay
  already exist, plus the config `linters` catalog and the job substrate. (Custom style linters =
  a natural sub-case feeding the existing diagnostics catalog.)
- **Direct-to-CMS publishing** (WordPress/Medium/Ghost) — command + background job + API keys.
  Reuses export/job substrate + config. Classic plugin.
- **Backlinks / wiki-links** (zettelkasten) — `[[link]]` index on the worker substrate + a
  backlink/follow overlay; composes with S2's directory model. Reuses outline-overlay-style list.

These are the sweet spot: *command + event hook + job + optional overlay/status*, none can
corrupt source (worst case = wrong count / failed publish).

### P-B. Custom-markup plugins — high value, cluster on ONE hard API need
<!-- item: PB -->

All three want the same capability — **plugins contributing custom inline/markup rendering** —
which is the trickiest P-API surface (rendering is core + per-frame, hot-path-adjacent). This
likely argues for a core **"markup-extension" mechanism** plugins register *into* (declare a
syntax + a face), rather than plugins rendering raw. Design deliberately in P.

- **Track Changes via CriticMarkup** (`{++ins++}`/`{--del--}`/`{>>comment<<}`) — THE feature
  keeping pros tethered to Word, in its plain-text-native form; pandoc already maps CriticMarkup
  ↔ docx tracked changes. Bridges wordcartel into the editorial `.docx` substrate the report
  calls unavoidable. Needs: inline span styling + accept/reject transforms + an export hook.
  **Highest-value P-B item and the best forcing function for the markup-extension API.**
- **Fountain screenplay** (scene headings, cues, dialogue) — purely plain-text genre support that
  fits the identity (the report holds Fountain up as the ideal). Needs custom line/inline
  rendering + a pandoc/afterwriting export path.
- **Wiki-link rendering** — the visual half of the P-A backlinks plugin.

### P-C. Lower-fit / niche / principled
<!-- item: PC -->

- **AI continuation** — plugin-*only, on principle*. The report's evidence: the complaint isn't
  AI, it's *unavoidability* ("uninvited co-author"). Opt-in plugin is the only defensible stance.
- **One-click book design (Vellum-like)** — at most a thin plugin shelling to a pandoc/epub
  template; real book design is a GUI product, mostly out of scope.
- **Genre benchmarking (AutoCrit)** — needs a comparison corpus; heavy, niche. Plugin at most.
- **Ulysses sheet-library (no filenames)** — mild tension with "file over app" transparency (it
  *hides* filenames); a workflow plugin if ever, low priority.

### Not plugins (recorded for completeness)

- **Split-view / research-beside-writing pane** — window layout is a core rendering concern; a
  plugin could fill a pane's *content*, not create the split.
- **WYSIWYG fonts** — N/A on a terminal (themed rendering instead).

### P-API requirements this list implies (a checklist for Effort P)

The candidates collectively require the P API to expose: **event hooks** (save/edit/idle/open);
**buffer + metadata read** (content, word count, active doc); **safe edits via transaction**
(accept/reject, apply-fix); **jobs** on the worker substrate (analysis, network); **UI
contributions** — status-line (goals), list overlay (backlinks/findings, reuse diagnostics), and
the hard one, **inline markup rendering** (CriticMarkup/Fountain/wikilinks); **sidecar file +
network I/O** (streaks, caches, CMS). CriticMarkup is the item that most stresses — and therefore
best validates — the markup-rendering capability; treat it as a P design anchor.

## Cross-cutting notes

- **Testing synergy:** every item lands with e2e `Harness` journeys (menu state machine,
  wrap/caret, palette-completeness invariant) and the PTY smoke layer covers mouse/real-
  terminal behavior (dwell-reveal is smoke-testable via tmux mouse events). The new
  infrastructure makes each of these cheap to pin.
- **Keyboard-reachability guarantee:** every mouse affordance has a keyboard path — enforced
  by the palette contract.
- **Preset-aware hints:** binding hints in menu/palette must re-resolve on keymap switch (A5).

## Resolved decisions (2026-07-03)

1. **Menu-bar default mode = `auto`** (dwell-reveal). (A1)
2. **Right-edge bar content: none by default** — background fill only; content designed in E1. (A2)
3. **Empty-selection transforms = block under the caret**, + explicit `_buffer` variants. (C2)
4. **Selection snapping = deepest enclosing block(s)**, not top-level; applies to the caret
   default too. (C2)
5. **Export-time typography adopted:** `export.typography = true` default ON; `false` → strict
   literal (`-smart`). (C1)
6. **No global Alt accelerators** — dropped; revisit on real demand; within-menu mnemonics =
   optional A1 garnish. (A4)
7. **Status line = transient chrome in full-zen** — auto-reveal on any message; never
   hide-outright. (E1)
8. **Heading glyphs default ON for ALL themes**, config opt-out, eyeball pass per theme. (B3)
9. **Menu curation principle adopted** (see the contract section); the item-by-item pass +
   judgment calls happen in the A3 effort. (A3)

## Still open (deliberately)

- The A3 item pass itself (applies decision 9; judgment calls come back for approval).
- Right-edge bar content design + full-chrome composition (E1).
- D1-a vs D1-b write-back (D1-b favored, not yet committed — settle at D1's brainstorm).
- Dwell duration and reveal/grace timings (implementation tunables, not design forks).

## Newly-tracked items (stubs)

*(Auto-created during the backlog-manifest migration. Status/size/kind live in `backlog.toml`; flesh out the triage prose here when the item is picked up.)*


### B9 — Menu bar horizontal overflow — clip/windowing for narrow terminals (<62 cols)
<!-- item: B9 -->

**Surfaced by the command-surface curation effort (2026-07-10, Task 6.1 verify).** That effort added
two menu categories (`Block`, `Documents`), growing the menu bar to 8 categories ≈ **62 columns**
(`File 6 + Edit 6 + Block 7 + Format 8 + View 6 + Documents 11 + Settings 10 + Export 8`).
`chrome_geom::menu_bar_layout_cats` has **no horizontal windowing** (only the dropdown has *vertical*
windowing) — so below ~62 cols the trailing categories clip: verified at 60×24 the bar renders
`… Settings Expor` (Export's label loses its last char) and Export's right-anchored dropdown renders
clipped to the right-edge column, i.e. **mouse-unreachable / unreadable** for the clipped tail.
**Keyboard reach is intact** (F10 + Left/Right cycles to and opens the clipped category via
`menu::intercept`), so this is a cosmetic/mouse degradation, not data loss — which is why it was
accepted for the effort's merge and filed here rather than expanding that effort's scope.

**Direction (when picked up):** add horizontal overflow handling to `menu_bar_layout_cats` — either a
scrolling/windowed bar (shift the visible category window to keep the active category on-screen, mirror
the dropdown's `list_window` approach) or a right-edge overflow affordance. Anchors:
`chrome_geom::menu_bar_layout_cats`, `menu::intercept` (keyboard cycle), the dropdown `list_window`
(vertical-windowing precedent), the tiny-terminal guard.


### A15 — About command/menu item that shows the splash
<!-- item: A15 -->

An `about`/`show_splash` command (registered into the command surface, reachable from the
palette and a menu row — Documents? a new Help category?) that re-displays the E6 splash on
demand. Reuses the existing splash render; the net-new work is the command + its dismissal path.

**Open design question — modal vs. current dismiss-on-input splash.** The startup splash today is
a *transient* screen dismissed by any keypress (it yields to the editor the moment you type). An
on-demand About is a different context: the user asked to see it, so it should stay until
explicitly closed (Esc/Enter), and it should not swallow the first keystroke into an edit. That
points at a **modal overlay** (like the palette/menu/diag overlays — an intercept in `reduce`,
painted over the canvas, Esc-to-close) rather than reusing the startup full-screen-until-input
path. Decide whether: (a) About is a distinct modal reusing only the splash *content* renderer;
(b) the startup splash itself becomes modal and About just re-invokes it; or (c) keep them separate
behaviours. Leaning (a) — startup wants dismiss-on-type (don't make writers press a key to start);
About wants dismiss-on-Esc. Same art, two dismissal policies.

*(Captured 2026-07-11 via `scripts/backlog add`.)*

### H19 — Clean recovery files offers an opened recovered-*.md dump for deletion
<!-- item: H19 -->

**Observation (Fable whole-branch gate, Effort B, 2026-07-11):** the H5 `Clean recovery files…`
command's enumerator protects an open buffer's *swap*, but if the user has opened a `recovered-*.md`
dump itself as a live buffer, that on-disk dump file is still offered for deletion. **No data loss**
(the dump content lives in the open buffer, whose swap IS protected, and a save re-creates the file;
`recovered-*.md` is already-extracted output the command is designed to clear) — so this is a latent
UX *surprise*, not a correctness bug. Polish: exclude a `recovered-*.md` from the deletable set when
it is the backing file of an open buffer (mirror the open-buffer-swap exclusion for dumps). Low
priority. Anchors: `swap.rs::cleanable_recovery_files`/`recovery_path_still_cleanable`, the
`open_swap_paths`/protected-set gather, `recovery.rs` (`recovered-*.md` naming).

*(Captured 2026-07-11 from the Effort-B Fable gate.)*

### H20 — Flaky test: filter::run_filter_non_zero_exit_carries_stderr
<!-- item: H20 -->

**Observed (v0.4.0 release gate, 2026-07-11):** `filter::tests::run_filter_non_zero_exit_carries_stderr`
failed in one of two back-to-back `cargo test --workspace` runs, then passed **10/10** on isolated
re-run — a genuine flaky test, not a regression (it predates 0.3.0; commit `7834562`, the filter
subprocess engine). A flaky test in the suite undermines the `cargo test` merge GATE (a real failure
could hide behind "probably just the flake," and a flake can spuriously block a merge). **Direction:**
find the race — likely a subprocess spawn/stderr-capture timing assumption on non-zero exit (stderr
read vs. child-exit ordering, or a too-tight timing/poll assumption) — and make the assertion
deterministic (wait on the condition, not a timing window; cf. condition-based-waiting). Low effort,
high hygiene value before Effort P. Anchor: `wordcartel/src/filter.rs`
(`run_filter_non_zero_exit_carries_stderr` + the sync subprocess engine it exercises).

*(Captured 2026-07-11 from the v0.4.0 release gate.)*

### E8 — Lens — the unifying view surface (layout vs style axes; plugin-registerable)
<!-- item: E8 -->

**A product concept, not a refactor (user, 2026-07-12): "a lens for your writing."** One first-class,
summoned, non-destructive way of *seeing* prose. It falls out of the arc's north star
(`docs/design/prose-structure-arc.md`): *show the writer the skeleton of their prose*. A lens is by
construction **summoned, non-destructive, and off by default** — already the law (arc D7; the E7
precedent: the cost lands in the summoned view).

> **⚠ REGROUNDED 2026-07-12 against SHIPPED code.** This item was first written pre-emptively, to
> constrain the then-in-flight multi-provider diagnostics effort. **That effort has since MERGED**
> (`a2f9062` — multi-provider diagnostics SPINE + switchable lens, ~4,300 lines). Its spec and plan
> **predate** its merge of this item, so E8's constraint did **not** reach its design. What follows is
> what the code actually does, not what E8 asked for.

### The real problem, stated from the code: FOUR toggle surfaces, no model

"How do I see my prose" is already answered four different ways, each with its own shape:

| Surface | Shape | Where |
|---|---|---|
| `RenderMode` (F1) — Draft / Preview / Source / Review | **exclusive cycle**; *also* the gate that summons diagnostics | `commands.rs`, `should_show_diagnostics` |
| `active_analysis_source: DiagSource` — which engine paints | **exclusive**, "one source painted at a time" | `diagnostics_run.rs::active_lens_diags` |
| `toggle_focus` + `focus_granularity` — dim all but the current sentence/paragraph | **boolean**, and it **STACKS** with diagnostics today | `render.rs::gather_row_ctx` |
| `toggle_typewriter`, `toggle_measure` | booleans | View menu |

S6 (a **layout** lens) and S8 (a **style** lens) would add two more shapes. **Nothing unifies them.**

### What SHIPPED, and what it does and does not cover

The merged "switchable analysis lens" is an **exclusive selector over diagnostic engines**:

```rust
// editor.active_analysis_source: DiagSource        (Harper | LTeX | Vale | Plugin(&'static str))
// "Every other engine's slot stays computed but invisible until the lens is switched onto it
//  (the locked never-merge decision: one source painted at a time)."
pub fn active_lens_diags(editor: &Editor) -> Option<&[Diagnostic]>
```

Command surface (contract rule 8, correctly): `analysis_engine_harper` (set-per-state primitive,
palette-only) + `analysis_next` (stateful cycle, View menu, state-in-label) + `toggle_engine_harper`
(per-engine enablement).

**Never-merge is RIGHT for diagnostics** — two engines squiggling the same span with different
underlines is a mess. Do not re-litigate it. **But it is not a lens surface**; it is an
*engine selector inside one*. It models neither a layout lens nor a non-diagnostic style lens.

**`DiagSource::Plugin(&'static str)` already exists** — plugin-declared engines are in the vocabulary.
Good.

### The design fork — and the code now VALIDATES it

**Lenses split on two axes, and they compose differently.**

- **STYLE lenses** change how text is **painted** — diagnostics, POS X-rays, repeated-opener
  highlighting. These **STACK**.
- **LAYOUT lenses** change what is **drawn** — S6's ventilate view inserts line breaks that are not in
  the buffer. These are **EXCLUSIVE**: two cannot re-break the same rows.

This is not a hypothesis. **The codebase already contains one of each, and they behave exactly this
way:** `toggle_focus` (a style lens — dims rows) **stacks** with the diagnostics underline today;
`RenderMode` and `active_analysis_source` are **exclusive**. The two-axis model is a description of
the code, not a proposal for it.

So the surface is **one active layout lens × N active style lenses** — not a single cycle, and not a
single checkbox set.

### The seam constraint (now a finding, not a warning)

**`DiagnosticsProvider` is whole-document, asynchronous, and process-lifecycled** —
`ensure_running()`, `shutdown()`, `availability()`, and `notify_change(buffer_id, version, path,
text: String)` taking the **entire document**. **S8's POS lens is caret-local, synchronous, and
processless** (harper-brill over the caret's block window, S7). Handing it a whole-document `String`
per check would violate the `O(visible) + O(edited)` rule outright.

**Conclusion: S8 must NOT implement `DiagnosticsProvider`, and should not try.** That is not a defect
in the shipped seam — a diagnostics provider genuinely *is* an async whole-document engine. It means
**S8 is a different lens KIND**, and E8's job is the **view / toggle / attribution surface above
both**, not one trait for all of them.

### Therefore: E8 is a GENERALIZATION ABOVE what shipped, not a reuse of it

The analysis-engine selector becomes **one lens family inside** a broader surface. That is a larger
E8 than first filed, and it must not be attempted by widening `DiagnosticsProvider`.

### Other things this unlocks

**Effort-P plugins should be able to REGISTER a lens.** A Lua plugin that dims every word over three
syllables, or highlights dialogue attributions, or marks every sentence over 30 words, *is a lens* —
tellable only if there is one seam to register into. (`DiagSource::Plugin` anticipates this for
*engines*; the lens surface needs the same for *lenses*.)

### Open questions for the brainstorm

- **`RenderMode` is doing double duty** — it is both the view cycle *and* the gate that summons
  diagnostics (`should_show_diagnostics`). Is a layout lens a **fifth `RenderMode`**, or is
  `RenderMode` itself really "the layout-lens axis" wearing another name? S6 must answer this.
- Does `toggle_focus` **become** a style lens under the new model, or stay a bespoke boolean? (It is
  the proof case that style lenses stack — it would be strange to leave it outside.)
- N style lenses = N palette toggles by contract rule 8 — but **what appears in the View menu**? A
  submenu? A cycle over "presets" of lens sets?
- **Attribution when several lenses paint the same span**: who wins, and can the writer tell which
  lens said what? (`render_status.rs` already attributes the engine — `REVIEW · Harper`.)
- Config namespace and persistence; interaction with the density presets (E1).

### B10 — EOF caret glued to last content line (shared caret_line clamp)
<!-- item: B10 -->

EOF caret glued to last content line (shared caret_line clamp)

*(Captured 2026-07-13 via `scripts/backlog add`; flesh out the triage prose when picked up.)*

### E10 — Multi-engine linting (b) — ltex-ls-plus / LanguageTool provider + JVM lifecycle
<!-- item: E10 -->

Multi-engine linting (b) — ltex-ls-plus / LanguageTool provider + JVM lifecycle

*(Captured 2026-07-13 via `scripts/backlog add`; flesh out the triage prose when picked up.)*

### E11 — Multi-engine linting (c) — diagnostics viewing/action delta (href, detail region, dict/rule writers, executeCommand)
<!-- item: E11 -->

Multi-engine linting (c) — diagnostics viewing/action delta (href, detail region, dict/rule writers, executeCommand)

*(Captured 2026-07-13 via `scripts/backlog add`; flesh out the triage prose when picked up.)*

### E12 — Multi-engine linting (e) — plugin-declared LSP servers + plugin-contributed engine-menu rows
<!-- item: E12 -->

Multi-engine linting (e) — plugin-declared LSP servers + plugin-contributed engine-menu rows

*(Captured 2026-07-13 via `scripts/backlog add`; flesh out the triage prose when picked up.)*

### PD — wc.async — one-shot subprocess primitive (formatters / vale-CLI); closed op-menu + AsyncDone pump-drain
<!-- item: PD -->

wc.async — one-shot subprocess primitive (formatters / vale-CLI); closed op-menu + AsyncDone pump-drain

*(Captured 2026-07-13 via `scripts/backlog add`; flesh out the triage prose when picked up.)*

### A18 — Snippet / abbreviation expansion (trigger to text; dynamic placeholders; cursor stop)
<!-- item: A18 -->

Snippet / abbreviation expansion (trigger to text; dynamic placeholders; cursor stop)

*(Captured 2026-07-13 via `scripts/backlog add`; flesh out the triage prose when picked up.)*

### A19 — Writing sprint — word-count goal + session progress (edge-triggered; NOT an elapsed timer)
<!-- item: A19 -->

Writing sprint — word-count goal + session progress (edge-triggered; NOT an elapsed timer)

*(Captured 2026-07-13 via `scripts/backlog add`; flesh out the triage prose when picked up.)*

### A20 — Forward-only drafting mode — toggle that disables deletion (Write-or-Die style)
<!-- item: A20 -->

Forward-only drafting mode — toggle that disables deletion (Write-or-Die style)

*(Captured 2026-07-13 via `scripts/backlog add`; flesh out the triage prose when picked up.)*

### PE — Bundled example plugins — full-featured writer plugins + authoring tutorials (each with a README)
<!-- item: PE -->

Bundled example plugins — full-featured writer plugins + authoring tutorials (each with a README)

*(Captured 2026-07-13 via `scripts/backlog add`; flesh out the triage prose when picked up.)*

### B12 — Lone block-begin marker renders nothing (^KB before ^KK is invisible)
<!-- item: B12 -->

Lone block-begin marker renders nothing (^KB before ^KK is invisible)

*(Captured 2026-07-13 via `scripts/backlog add`; flesh out the triage prose when picked up.)*

### B13 — Block markers — styled boundary cells (modern B-lite; no injected bracket glyphs)
<!-- item: B13 -->

Block markers — styled boundary cells (modern B-lite; no injected bracket glyphs)

*(Captured 2026-07-13 via `scripts/backlog add`; flesh out the triage prose when picked up.)*

**Anti-regrowth constraint (record for the spec + plan).** This effort adds new boundary
commands *and* new render arms — exactly the H1 dispatch-attractor shape. As of 2026-07-14 the two
guarded hubs sit close to their `module_budgets` caps: `app.rs` production ≈920/1000 (~80 lines
headroom), `render.rs` production ≈821/900 (~79). New command registration must land through the
registry rows / feature-module handlers, and the styled-boundary paint must land in a
`render_*`/feature module invoked by a thin arm — **not** as inline bodies grown into `reduce` /
`reduce_dispatch` / `place_cursor`. The spec AND the plan each state how they honor this (the
command-surface contract already binds the command half); the `too_many_lines` + `module_budgets`
GATEs enforce it, but with this little headroom, plan for the seam up front rather than discovering
it at the merge gate.

### S9 — In-lens editing feel — refine caret/motion/reflow inside the ventilate lens
<!-- item: S9 -->

**Origin (2026-07-14):** the S6 ventilate lens is a hit ("love love loving it") and STAYS A LENS
(non-destructive; do NOT convert to a RenderMode or a destructive transform — the toggle-off-is-byte-identical
property is valued). Open question the user wants to revisit **after more editing time in the lens**, once they
have firmer opinions on how it *should* feel — deliberately NOT designed yet. Banked so it isn't lost.

When a paragraph shows one-sentence-per-row but is one logical line underneath, "editing" splits into distinct
feels that don't all bother equally. Observations to test against real use:

- **Vertical motion** — should ↑/↓ move by *sentence-row* (what you SEE) or by *logical line/paragraph* (what's
  underneath)? The likeliest friction: see-and-*move* can disagree even where see-and-*select* already agrees
  (SEE==SELECT is a selection invariant, not a motion one).
- **Caret landing on the wrap** — where the caret lands horizontally when moving into a shorter/longer sentence,
  and whether the 6-col rhythm gutter shifts the sense of column.
- **Reflow while typing** — as a sentence is extended, or a period is added, its row-group reflows and a
  sentence-row can appear/disappear mid-keystroke. Smooth vs. jumpy.
- **Enter semantics inside the lens** — soft break vs. paragraph break, and how the display re-segments after.

Grounding when picked up: the window-aware resolver (`ventilate::resolve`), `nav` motions, and the
sentence-segmentation the lens shares with `select-sentence` (S5's detector). Slots as an S-theme follow-on to
S6. Scope only after the user names the specific moment it feels wrong — do NOT boil the ocean.

### B14 — Ventilate lens treats tables as prose (no Table BlockRole → prose_block_at never declines)
<!-- item: B14 -->

**Origin (2026-07-14):** surfaced by the S4 Codex spec-gate while grounding S4's SEE==SELECT decline
predicate against the SHIPPED S6 ventilate lens (`docs/backlog-archive.md#s6`). **Not an S4 bug — a
pre-existing gap in shipped S6.** The lens's prose test, `ventilate::prose_block_at`, declines a block
only when `role_at(byte) != BlockRole::Paragraph`. But `BlockRole` has **no `Table` variant** and
`kind_to_role` does **not** map `BlockKind::Table` to a non-paragraph role — so a markdown table
classifies as prose, and the lens **ventilates it** (segments its row text as "sentences" one per
row-group + a rhythm gutter), which is wrong: a table is not prose.

**Consequence for S4 (why it was found here):** S4's `select_sentence`/mutations decline exactly what
the lens declines (SEE==SELECT), so S4 inherits this — a table currently reads as prose to
`select_sentence` too. S4 (spec `2026-07-14-s4-prose-surgery-design.md`, decision F3-A) deliberately
scoped this OUT: it declines the lens's *current* set (heading / list / code / blockquote / front-matter
/ comment) and treats tables as prose to stay literally SEE==SELECT, rather than reach into core block
classification mid-effort. So this item is the elevation of that deferred piece back to its true home
(S6/core).

**Fix direction (when picked up):** give tables a non-paragraph identity so BOTH the lens and any
SEE==SELECT consumer decline them — add a `BlockRole::Table` (or fold into an existing non-prose role)
and map `BlockKind::Table` in `kind_to_role` (`wordcartel-core/src/style.rs`), then confirm
`prose_block_at` (`ventilate.rs`) declines it and the lens renders the table verbatim. Small core change;
re-verify the lens's no-op-when-off invariant and add a table fixture to the ventilate tests. Low
priority (cosmetic: a ventilated table is ugly but non-destructive; toggle off → byte-identical).
Grounding anchors: `ventilate::prose_block_at`, `kind_to_role` + `BlockRole` (`style.rs`),
`BlockKind::Table` (`block_tree.rs`).

### B15 — Shrink into a folded region leaves the caret on a hidden line (no SnapOut)
<!-- item: B15 -->

**Origin (2026-07-14):** deferred Minor from the S4 whole-branch final gate (Fable ruled GO-compatible,
not a merge blocker). S4's F4-A stateless `ShrinkSelection` deleted the old pop-based
`place_caret_visible(SnapOut)` along with `sel_history`. Fable's probe confirmed: when the current
selection's `from()` is **already inside a folded body** (selection made before folding, or extended
into a fold), a shrink can leave the caret on a `FoldView::is_hidden` line — typing would edit invisible
text. Esoteric precondition (every natural ladder path evaluates at a canonical rung's `from()` and lands
on visible bytes; symmetric with `ExpandSelection`, which never snapped).

**Fix direction:** apply the SAME guard the S4 I-2 fix added to `block_move`/`swap` —
`registry::snap_caret_out_of_fold` (`place_caret_visible(.., CaretPlace::SnapOut)`) — AFTER
`ShrinkSelection` re-derives, so the caret snaps out of any fold. **Must stay stateless** — do NOT
re-introduce `sel_history` (that would undo F4-A). Low priority. Related: S4 T3 (F4-A stateless shrink),
the S4 I-2 block_move/swap snap guard. Anchors: `commands.rs::ShrinkSelection`,
`registry::snap_caret_out_of_fold`, `fold::normalize_caret`.

### B16 — Scope::Sentence highlight window drifts from content-anchored select on indented prose
<!-- item: B16 -->

**Origin (2026-07-14):** found by the S4 whole-branch Fable review — but **PRE-EXISTING** (present at
the S4 merge base `ef03888`, NOT introduced by S4). S4 made `select_sentence` + the mutations
content-anchored (via `commands::prose_window_at` → `ventilate::line_content_byte`), so SELECT now agrees
with the S6 lens on indented prose. But the **active-sentence HIGHLIGHT paint** (`render.rs` ~505-508,
the `Scope::Sentence` render path) still derives its window from the **raw** `nav::paragraph_range_at(head)`.
So with the caret in a ≤3-space CommonMark indent, the *painted* active-sentence region diverges from what
`select_sentence` actually selects — a SEE==SELECT violation on the PAINT side that S4 left standing
(outside its blast radius).

**Fix direction:** route the highlight window through the same content-byte anchor the select/mutation
path uses (`prose_window_at` / `line_content_byte`) so the painted region matches the selection. Small,
localized to the `Scope::Sentence` paint arm. Related: S4 (content-anchored select), C-11
(content-byte classification). Anchors: `render.rs` `Scope::Sentence` highlight (~:505),
`commands::prose_window_at`, `nav::paragraph_range_at`.

### C5 — File interface: unify save/write onto the picker + favorites/recent
<!-- item: C5 -->

**Observation (user, 2026-07-15):** file operations are UX-asymmetric. **Open** uses the rich
`file_browser` **modal** (navigate dirs, `..`, substring-filter, select). **Save As** (`prompts::open_save_as`
→ `MinibufferKind::SaveAs`) and **Write Block** use a **blind minibuffer text entry** — you type the path with
no directory browsing, no visibility into what's already there, no favorites; overwrite is a confirm `Prompt`.
Export target is the same. So **reading a file gets a picker; writing one makes you type a path blind.** Today's
`file_browser` is minimal: open-only (list/filter/nav/select), **no save mode, no favorites, no recent, no
projects, no create-new**.

**Motivation:** writers are increasingly not CLI-native. The `~/projects/wordprocessing/beloved-features-report.md`
survey names the target directly — Ulysses' library ("*no file names, no Finder management…*") and Scrivener's
Binder are the most-cited beloved features; abstracting raw path management is the gap.

**Scope — layers (this item = Layers 1–2):**
- **Layer 1 (tactical, high-value):** give `file_browser` a **save/write mode** — a filename field + directory
  nav — so **Save As**, **Write Block**, and the **export target** route through it instead of blind minibuffer
  entry. Closes the asymmetry; mostly extends existing infra.
- **Layer 2:** **favorites/pinned directories** + **recent files/dirs**, surfaced in the picker. Needs a
  config/persistence surface and command-surface conformance for any settable list.
- **Layer 3 (NOT this item):** "**projects**" (a manuscript root / working dir) = **S2** (directory-as-binder) +
  the still-open **writing-unit question Q6** (single long doc vs book-as-directory), composing with **S1**.
  Do not build "projects" ad-hoc in the picker — favorites are the cheap on-ramp; S1/S2 own the manuscript model.

**Build-not-buy (decided 2026-07-15, same logic as A17 / C3):** extend the existing `file_browser` + the A6
windowed-list painter + the `theme_picker`/palette pattern — do NOT adopt a file-picker crate. A crate re-does
the easy part (list a dir) that's already done, and can't honor the theme/chrome elevation ladder,
terminal-plain fallback, the command-surface contract (keymap/hints/palette), or no-silent-UI; dep-weight is an
active constraint and the core is `#![forbid(unsafe_code)]`. The value is the writer-first integration
(favorites/projects/persistence), which no generic widget provides. (Small utility crates for a *piece* —
fuzzy-matching, path completion — are separately evaluable, but not a whole-picker dependency.)

**Prior-art note:** the editor read *at source* for A17 was `sinelaw/n` (its messaging system, not its file ops);
its file-operation internals are NOT captured here, and "Fresh" (as the user named it) is unresolved — research
`n`/Fresh's picker if a concrete reference is wanted.

**Relationships / sequencing:** a richer file interface adds overlay surface → land **after / conforming to
`H21`** (the overlay-dispatch table) so it registers into the seam rather than adding another hand-parallel
overlay (reinforces H21's value). Config/persistence surface for Layer 2. `S2`/`S1`/**Q6** own Layer 3.

**Size:** M for Layers 1–2 (save-mode picker + favorites/persistence + command-surface wiring); L if it grows
toward projects (which should be S2 instead). Good candidate for the Fable-first, writer-first pipeline.

### H23 — palette_overlay_rect u16 overflow at extreme terminal width (H7-class geom)
<!-- item: H23 -->

**Surfaced by the H21 whole-branch Fable probe (2026-07-16), Minor — PRE-EXISTING, not introduced by H21**
(verified byte-identical on `main` at 1c8d10e). `chrome_geom::palette_overlay_rect` computes the overlay width
as `let ov_w = (w * 3 / 5)…` in `u16`; the intermediate `w * 3` overflows `u16` for a terminal width `w ≥ 21846`
— a debug-build panic, or a release wrap that then clamps. Fable's degenerate-area probe (300×100 and extreme
coordinates) hit it. Reachable only via an absurd/hostile terminal `Resize` (no real terminal is ~21846 columns),
so it is **not a data-loss or normal-use hazard** and was **not a merge blocker** for H21 — but it is the same
arithmetic-soundness class H7 audited (widen the intermediate, or `saturating_mul`/`u32`). **Fix:** compute the
`* 3 / 5` in `u32` (or use `saturating_*`) and clamp back to `u16`, in `chrome_geom.rs::palette_overlay_rect`;
sweep the sibling `*_overlay_rect` helpers for the same pattern while there. Anchor: `wordcartel/src/chrome_geom.rs`
(`palette_overlay_rect` and the other overlay-rect helpers it shares the `w * k / n` shape with).

**Scope when picked up — this IS an H7-style geom sweep, not a one-line fix.** Treat the `palette_overlay_rect`
overflow as the *seed/exemplar*, not the whole item: when H23 is worked, audit every geometry helper (the
`chrome_geom.rs` rect functions and any `w * k / n` / `area.width *`-shaped arithmetic in the layout/render path)
for the same overflow/underflow class H7 covered. Apply H7's **blast-radius stance** ([[wordcartel-h7-blast-radius-stance]]):
these are **render/geometry (parse-class) paths**, not mutation paths — so the guard is `debug_assert` + a **safe
release clamp** (compute in `u32`/`saturating_*`, clamp back to `u16`), *not* a loud release panic and never a silent
garbage wrap. Small and cold, so it batches cleanly as its own mini-sweep effort or as a rider on the next
hardening pass — but the *scope is the sweep*, and the fix pattern (widen-compute-then-clamp under a debug_assert)
is already decided here so it's ready to go.

*(Captured 2026-07-16 from the H21 final Fable gate. H7-sweep framing recorded 2026-07-16.)*



### H25 — compose::face_to_ratatui is add-only — can't express modifier subtraction
<!-- item: H25 -->

**Surfaced by the chrome-selection-legibility effort (B7, Codex spec gate 2026-07-17, correcting the
first framing).** `compose::face_to_ratatui` (`wordcartel/src/compose.rs`) is **add-only**: it does
`add(face.dim, Modifier::DIM, s)` — emitting `Modifier::DIM` for `Some(true)` — and has **no path to
emit a ratatui `sub_modifier`**. Consequence: a `Face`'s `dim` can never *subtract* a DIM that an
underlay (an earlier `set_style` on the cell) already wrote. Note this is NOT "`dim = false` is
ignored": `Theme::override_face` DOES honor `Some(false)` (`if patch.dim.is_some() { f.dim = patch.dim; }`),
so a face's own `dim` is set correctly — but compose still can't produce a style that *clears* an
inherited modifier. Ratatui itself supports subtraction (`Style::remove_modifier` / `sub_modifier`,
honored by `Cell::set_style`); the gap is purely in wordcartel's compose layer.

**Not a live bug today** — the B7 fix strips the leaked DIM at the `ChromeStyles::build` cache seam via
`.remove_modifier(Modifier::DIM)` on the already-composed style, which does NOT depend on this. H25 is
the deeper, more honest fix that B7 deliberately did NOT take (scope creep for a cosmetic bug): teach
`face_to_ratatui` to express modifier subtraction (e.g. a `Face` clears an inherited modifier), so the
strip needn't live at the cache seam and the model generalizes beyond DIM. Small, but touches core
compose semantics — do it as its own considered change, not folded into a cosmetic fix. Anchors:
`compose::face_to_ratatui` (add-only path), `Face.dim` tri-state, `override_face` (honors `Some(false)`),
`ChromeStyles::build` (where B7 works around it). ~S.

*(Captured 2026-07-17 via `scripts/backlog add`; reframed same day from the B7 Codex spec-gate correction.)*

### S10 — Prose objects — Phrase/Clause select-only + D5 clause-splitting
<!-- item: S10 -->

**The arc tail — deferred from S8.** S8 (prose lenses, shipped 2026-07-18) delivered the reusable
prose-lens spine + the four flag-word/pattern lenses. This item is the scope the S8 spec
(`docs/superpowers/specs/2026-07-17-s8-prose-lenses-design.md`) explicitly carved off to a follow-on —
the spec calls it "S9" throughout, but that id was already taken by an unrelated item, so it is filed
here as **S10** (the spec's "S9" references are nominal — they mean this).

**Scope (both SELECT-ONLY — arc law D6: analysis may COLOR + SELECT, never MUTATE without a visible
abortable selection):**
- **`Phrase` object** — the chunker's noun-phrase runs. The substrate already exists: `TokenTag.np`
  (from `wordcartel_nlp::analyze`) flags NP membership per token; S8 stored it but paints/selects nothing
  with it. This item turns NP runs into a selectable object (select the current/next noun phrase).
- **`Clause` object — D5 clause-splitting.** POS-informed clause boundaries (CCONJ / SCONJ / ADP
  disambiguate the coordinator vs subordinator vs preposition senses of for/so/yet), exposed as a
  selectable object. **D5 THE LAW:** clause splitting ships SELECT-ONLY behind a **measured precision
  gate** — Brill is newswire-trained and WILL mistag fiction / dialect / verse, so it must never mutate,
  only select, and only after its precision is measured on real prose (mirror the S4/D5 stance).

**Reuse:** builds directly on S8 — the `PosStore` + `PosSweep` substrate, the range-select nav pattern
(`prose_lens_next_match`), and the `wordcartel-nlp` classifier surface. Not yet designed; graduates to
the gated pipeline (brainstorm → spec → gates → plan → build) when picked up.

### H26 — fs-chokepoint guard: use-tree parsing for full soundness
<!-- item: H26 -->

C5 ships `wordcartel/tests/fs_chokepoint.rs`, a merge gate that fails the build when production code
reaches the filesystem outside the seam. It is a **textual scanner over three detection layers** —
import-gating (`use std::fs…`), fully-qualified `std::fs::` paths, and the closed std-defined set of
filesystem-touching inherent `Path` methods in both dot-call and UFCS spellings. C5's spec states its
coverage as an explicit Caught / Not-caught pair rather than claiming soundness.

**The gap.** Three import spellings evade layer 1 and are documented as accepted limits:

- nested grouped imports — `use std::{fs::File as StdFile};`
- renamed-in-group imports — `use std::{fs::{self as filesystem, OpenOptions as OO}};`
- leading-root paths — `use ::std::fs;`

Each enables a bare short-form call (`filesystem::write(…)`) that no specified layer sees. Closing
them properly requires **parsing `use` trees** rather than matching text — a dev-dependency (`syn`
or equivalent) plus real parsing logic in a test.

**Why it was deferred, not overlooked.** Weighed during C5's spec gate (2026-07-18) and declined as
disproportionate: the gate exists to catch the realistic regression — someone mid-effort reaches for
`fs::read_to_string` and writes an ordinary import — and the gap spellings are ones nobody produces
by accident. Adding a parser dependency also cuts against C5's zero-new-dependencies decision. The
scanner's own self-check plants one evasion per detection route, so the layers that exist are proven
to work; what is unproven is only the undisclosed-spelling case.

**Note the boundary that is NOT part of this item.** fd-originated `fs::File` (`From<OwnedFd>`,
`FromRawFd`) is outside the chokepoint rule by design, not by scanner weakness — the rule governs
reaching the filesystem *by path*, and a descriptor names no path. If a future effort starts doing
fd-based filesystem work, the **rule** needs widening, not the scanner.

**When it becomes worth doing:** if a real bypass ever lands via one of the three spellings, or if
the seam's guarantees start carrying weight they do not today (a plugin FS API, sandboxing, an
audit requirement). Absent that, the honest-limits gate is the better trade.

*(Captured 2026-07-18 during C5's spec gate.)*

### H27 — dispatch signatures: pass DispatchCtx instead of 8 loose args
<!-- item: H27 -->

Seven dispatch functions now carry `#[allow(clippy::too_many_arguments)]`, added by C5 Task 5 when
one new parameter (the `Arc<dyn Fs + Send + Sync>` seam handle) pushed each from 7 args to 8:
`input::handle_key`, `app::reduce`, `app::reduce_dispatch`, `app::dispatch_overlay_command`,
`menu::dispatch_row_action`, `mouse::handle`, `plugin::pump::drain_one_dispatch`.

**Why this is a real item rather than bookkeeping.** Two of those functions take the eight loose
arguments and then **reconstruct the bundle from them** a few lines into the body — `reduce_dispatch`
receives `(msg, editor, reg, keymap, ex, clock, msg_tx, fs)` and immediately does
`let ctx = DispatchCtx { reg, keymap, ex, clock, msg_tx, fs };`. The bundle is not missing; it is
being disassembled at the call site and reassembled in the callee.

**The fix is unusually clean, because `DispatchCtx` was built for exactly this.** It already carries
`reg`, `keymap`, `ex`, `clock`, `msg_tx`, `fs` — precisely the loose parameters — and it
**deliberately excludes `&mut Editor`** so the editor can be passed separately without an aliasing
tangle in the H21 table loop. That is the whole reason its shape is what it is. So the signature
collapses to three arguments and the allow disappears:

```rust
reduce_dispatch(msg, &mut Editor, &DispatchCtx)
```

This is using a bundle the codebase already has, for the case it was designed for — not a refactor
looking for a justification.

**Why it was deferred out of C5 rather than folded in.** Re-signaturing these is cross-cutting, and
21 of C5's 26 task briefs were written against the loose form. Changing it mid-effort would
invalidate briefs that fresh implementers — who see only their own task — have no way to
cross-check against.

**Why it should not simply be dropped.** The project's own history is the argument. The H1
god-object split (`app.rs`/`render.rs`, 2026-07-09) happened because *dispatch attractors* accumulate:
a central `match` that every feature must edit grows monotonically with feature count.
`clippy::too_many_lines` and `too_many_arguments` are the anti-regrowth GATEs that exist to catch
that class early. Seven suppressions is the lint doing its job and being told to be quiet — and each
one is individually defensible, which is precisely how the previous accumulation happened.

**Scope (expected Small):** collapse the seven signatures onto the existing context bundle; delete all
seven allows; confirm `module_budgets` and `too_many_lines` stay green. No new types, no design forks
— the bundle exists and every call site already holds its pieces.

**Anchors:** `overlays::DispatchCtx` (`overlays.rs`), `registry::Ctx` (`registry.rs`),
`app::reduce_dispatch` / `app::dispatch_overlay_command` (`app.rs`), the seven allow sites (grep
`too_many_arguments`). Precedent for the bundle's editor-exclusion rationale: the H21 overlay
dispatch-table effort.

*(Captured 2026-07-18 from C5 Task 5's review. See also [[H1]] — the god-object split this lint
exists to prevent recurring.)*

### H28 — Un-pumped picker tests assert unreachable states
<!-- item: H28 -->

Two tests — `save_as_empty_path_is_a_sticky_warning` and its Write-Block twin — pass only because
they act on the picker **before pumping the async directory listing**. Once a listing lands on any
non-root directory the warning they assert becomes unreachable, so they assert a state real usage
never reaches.

This is the class that hid a Critical through the whole C5 effort: every picker test pressed Enter
without pumping, so a bug that **descended into the parent instead of saving** survived ten
plan-gate rounds and twenty task reviews. The convention is now "pump the listing, drive the real
intercept" — these two are the last holdouts.

Either make them reachable (assert the warning in a state a writer can actually be in) or retire
them. A test that passes for the wrong reason is worse than no test: it reports coverage of a path
nobody is checking.

### H29 — recovery::LAST_GOOD process-global race makes the test gate non-deterministic
<!-- item: H29 -->

`editor::tests::undo_and_redo_refresh_the_recovery_snapshot` fails intermittently — roughly 2 runs
in 14 — on a race over the process-global `recovery::LAST_GOOD`. `git log -S` places it on `main`
since **H22**; it is not a C5 regression, and C5 deliberately left it alone.

It matters because it makes `cargo test` — a merge GATE — non-deterministic. Every effort since H22
has had to tell its implementers and reviewers "this one is a known flake, re-run it, don't chase
it," which is exactly the instruction that trains someone to re-run a *real* failure instead of
reading it.

Two sibling flakes fire only under parallel load and pass at `--test-threads=1`:
`config::tests::files_type_filter_unknown_warns_and_defaults_documents` and
`filter::tests::run_filter_non_zero_exit_carries_stderr` (the latter has a genuinely load-sensitive
10s spawn budget). Worth triaging together — the fix for a process-global race and the fix for a
timing budget differ, but the symptom a contributor sees is identical.

### A22 — Write-Block Redirect exports the whole document, not the marked block
<!-- item: A22 -->

In Write-Block mode, choosing a destination whose extension pandoc can produce (`excerpt.html`,
`report.docx`) redirects into the Export flow — and Export then exports the **whole document**, not
the marked block the writer was working with.

Pre-existing for a *typed* destination; C5's Row-2 format protection now also reaches it when the
writer highlights an existing foreign-format file. It is not silent — the picker title changes to
"Export" — so a writer who is reading has a cue. But the mode they started in promised a block
operation, and what they get is a whole-document one.

Two candidate resolutions, and the choice is a product call: either Export honours the marked block
when the flow was entered from Write-Block, or the redirect states plainly that it is leaving block
scope. Related: a Row-2 confirm onto a `.docx` target does not currently say the write would be
plain markdown.
