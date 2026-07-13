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

### B7. Selected menu-item text too light / less legible
<!-- item: B7 -->

**Observation (user):** the text of the HIGHLIGHTED (selected) menu item is too light. It "used to be dark"
(more legible); the user suspects the E5 dimming treated all menu text uniformly, hurting the selected
item's legibility, and asks whether the selected item should get a distinct highlight color.

**Grounded (may drift) — filed as a POTENTIAL BUG (possible regression from E5, shipped this session):** the
selected menu item uses the `ChromeSelected` face — "explicit fg/bg selection (menu item — today
Black-on-White, NOT reverse)" (`theme.rs:37`), and `derive_chrome` marks it "inverted highlight —
UNCHANGED" (`theme.rs:332`). On paper E5 (which receded/dimmed the `Chrome` BAR face, `5e1c2ea`) did NOT
touch `ChromeSelected` — so if the selected text really went dark→light, the cause is subtler than a direct
E5 edit and needs investigation. Candidates: the dropdown NORMAL items use `ChromeMuted` + DIM, and the
selection may be drawn as a bg change that leaves the dim fg in place rather than swapping to
`ChromeSelected`'s dark fg; or a compose-order interaction. Two directions the user raised: (a) give the
selected item a dedicated highlight fg color; (b) at minimum restore dark, legible selected-item text.
Anchors: `ChromeSelected` (`theme.rs:37,332`), the dropdown/selected-item render path (`render.rs` menu
paint), `ChromeMuted` (dropdown normal), E5 (`derive_chrome` recede, `5e1c2ea`).

### B8. Configurable terminal text-caret shape / colour
<!-- item: B8 -->

**Idea (user):** let the user choose the colour and size/style of the terminal text caret (block vs beam vs
underline, blink, colour) — "some people have opinions on the caret they use/see."

**Grounded (may drift):** the app does NOT set the terminal cursor shape today (no `DECSCUSR` /
`SetCursorStyle` / OSC-cursor-colour emission in the tree) — it leaves the caret to the terminal default.
Adding it means emitting `DECSCUSR` (`CSI Ps SP q`: 1/2 block, 3/4 underline, 5/6 bar; blink vs steady) on
startup/focus and restoring on exit, and optionally OSC 12 cursor-colour set/reset — plus a user-settable,
persisted style option (command-surface). Caveats: terminal support varies; tmux passthrough; MUST restore
the caret on exit/suspend/panic (mirror the panic→restore path). Anchors: crossterm cursor APIs, terminal
setup/teardown (raw-mode enter/leave + the panic-restore hook), the command-surface option pattern.

---

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

### A16 — Format menu: drop redundant Transform entry
<!-- item: A16 -->

**Observation (user):** the `Format` menu carries a `Transform` entry that is redundant — the menu
already exposes the underlying options list, so `Transform` duplicates a door that's already there.
Drop the `Transform` menu row.

**To ground when picked up:** confirm what the `Transform` menu row actually invokes (the
transform-scope cycle — Reflow/Unwrap/Ventilate, C2 — vs. a submenu) and which options rows it
overlaps with on `Format`, so removal drops only the duplicate affordance and not a unique path.
Command-surface note: this is a **menu curation** change (law 4, menu ⊆ palette) — remove the
`menu: Some(Format)` tag from that command, not the command itself; it stays palette-reachable.
No behaviour or keybinding change. Pairs with the A3b curation lens.

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

### A17 — Messaging / notification system — routed, browsable, plugin-emittable
<!-- item: A17 -->

Design a **first-class messaging/notification system**: a single routed path every user-facing
message flows through, with structured *kinds*, a browsable history, and a plugin-facing emit API —
so that Effort-P plugins (and the host itself) talk to the user through one well-designed surface
instead of ad-hoc status-line pokes. **This is load-bearing for a more customizable interface**: when
messaging is a real system rather than a single status string, users can configure *how* messages
reach them (where each kind routes, transient vs. sticky, quiet vs. loud, history retention), and
plugin authors get a contract to build against. Wants to be **really well thought out** — it defines
the interaction texture of the plugin era.

**Motivating example — Neovim's `noice.nvim` (the anti-pattern to learn from, NOT to copy).** Noice
is beloved because Neovim's core messaging is weak: blocking `hit-enter` prompts, messages that scroll
off and vanish, the cmdline doing triple duty, no routing by kind/source, no good history. Noice can
only fix that by **intercepting the `ext_messages` UI event stream and re-rendering everything over the
top** — it *overrides* the native path because *participating* in it isn't possible. The lesson is
**properties vs. mechanism**:
- The **properties** noice delivers — messages structured and routed by kind/severity/source,
  non-blocking, a browsable history, plugin-addressable — are exactly what a plugin-era editor wants.
- The **mechanism** (a plugin monkey-patching over the host's message path) is precisely the
  anti-pattern our architecture exists to prevent. In our world a plugin **registers a
  sink/renderer into a messaging seam** (Open–Closed, same story as timers/commands/diagnostics
  providers) and **emits** into the routed system — it never rips out and replaces the host path.
  Noice exists *because* core messaging was under-designed; designing ours right means no
  "noice-for-wcartel" ever needs to exist.

**Where we already stand (ahead of pre-noice Neovim on the worst sins):** "no silent UI waits" already
forbids blocking hit-enter prompts by charter, and typed errors already route to the **status line**
(no console — the app owns the terminal). What we almost certainly *lack* is the structured layer:
message *kinds*/severity, transient-vs-sticky lifetime, a `:messages`-style **history**, and above all
a **plugin-facing emit API**. This is a peer surface to the status line / palette / menu (hence Theme
A) and is governed by the same "one source of truth, surfaces derive from it" spirit as the
command-surface contract.

**Questions to resolve when picked up (Effort-P design input — decide *with* P, or just ahead of it):**
- **Kinds/severity** — info / warn / error / progress, and how each routes (status line vs. transient
  toast vs. sticky vs. history-only). Exhaustive enum, not stringly-typed.
- **Lifetime** — transient vs. sticky; who clears each, and how clearing stays edge-triggered (no
  wall-clock timers on the idle path — cf. the swap-thrash class).
- **History** — a browsable message log (`view_messages` command?) so a flashed-and-vanished message is
  recoverable. Neovim's single biggest day-to-day loss.
- **Plugin contract** — plugins *emit* (and optionally *register a renderer* into the overlay seam);
  they never override the host path. Rate/spam containment so a chatty plugin can't drown the surface.
- **User customization** — the payoff: per-kind routing/verbosity/retention as user-settable options,
  which per the command-surface contract means each is a command with one shared setter.
- **Command-surface conformance** — any surfaces/toggles this adds (history view, per-kind routing)
  must be palette-exhaustive commands with hint-tracking; state this in the eventual spec AND plan.

**Anchors (map when picked up):** the status-line render path, the typed-error → status-line surfacing
(`SaveError`/`OpenError`/`EditError`), the overlay seam (menu/palette/prompt render), and the Effort-P
design-space doc (`docs/design/effort-p-plugin-system-design-space.md`) — this belongs in P's surface.

**Prior art — Fresh (`sinelaw/fresh`), read at source 2026-07-11.** A mature Rust/ratatui editor
whose messaging is instructive precisely because it is *fragmented* into three disjoint subsystems —
a single status string, a "warning-domain" indicator system, and two tracing-backed log files — with
**no unified message type**. That fragmentation is itself the lesson: A17 should define the ONE
`Message { severity, kind, text, lifetime: Transient|Sticky, source: Host|Plugin(id), actions }` that
Fresh never unified, consumed by the status line, the history log, and any panel.

*Worth stealing:*
- **`tracing` target as the history spine.** Every user message emits `tracing::info!(target:"status",
  …)`; a `Layer` tees it to a file opened as a read-only buffer. That yields the browsable history
  (a `view_messages`) essentially for free, cleanly split (functional-core emit / shell layer).
- **The warning-domain trait.** A `WarningLevel` + a `WarningDomain` trait (`label`/`level`/
  `popup_content` carrying *typed actions* — `ViewLog`/`Dismiss`/`CopyToClipboard`/`Custom`) + a
  registry that aggregates N sources to one indicator via `highest_level()`. A clean *sticky-with-
  actions* tier distinct from transient flashes; `Custom(String)` is a ready plugin seam; their LSP
  domain even synthesizes install-command hints (a diagnostic that offers a *fix*, not just a
  complaint).
- **Zero wall-clock display timers** (their accidental win). Messages clear edge-triggered (next
  write / explicit dismiss). Preserve this deliberately: any transient tier must clear edge-triggered
  (next input / after N frames), never an idle clock — keeps us clear of the swap-thrash bug class.

*Anti-patterns to avoid (each sharpens A17's thesis):*
- Plugin `setStatus` **silently overrides** the host status line (a `plugin_status_message` field that
  shadows the core one) — the exact override-anti-pattern A17 exists to forbid. Fix: route plugin
  emits into the SAME queue with a `source: Plugin(id)` tag and let *our* policy decide precedence —
  never a shadowing field.
- **String-sniffing for plugin errors** (a `"js error"` substring match) — use a typed severity on
  the emit API instead.
- **No rate-limit on the plugin emit path** — a looping plugin repaints the status line every frame.
  Put the throttle/dedup on *emit*, not just the history file (Fresh only dedups the log).
- **Severity-less `setStatus(string)`** — bake `severity` and `sticky` into the emit API signature
  from day one; retrofitting is *why* Fresh's warnings and status are two disjoint systems.
- They pushed the aggregated diagnostics **list** down into a plugin; their own eval rates it 3/10
  with three criticals, all at the host/plugin mode-composition seam. Keep any first-class browsable
  list in core with a real focus/keymap contract.

*Gap we can beat:* both Fresh logs truncate per-session and grow **unbounded within a session** — give
our history a bounded in-memory ring (M5 resource-cap ethos) + optional file spill.

*(Captured 2026-07-11 from a noice.nvim discussion; prior-art subsection added the same day from a
source read of Fresh. Triage — not yet scoped or sized; explicitly flagged by the user as needing to
be really well thought out because it shapes the customizable plugin-era interface.)*

### S4. Prose text objects — structural selection + operator layer
<!-- item: S4 -->

**RE-SCOPED 2026-07-12** (was XL) after a code-grounding pass + an independent Fable review. It is
now one item in a multi-effort arc — **S5 → S6 → S4 → S7 → S8** — whose umbrella document is
`docs/design/prose-structure-arc.md`. **Read that first**; it carries the north star, the measured
decisions, and the cuts. (The original idea material,
`docs/design/prose-text-objects-design-space.md`, was drafted by an external LLM with **no codebase
access** and several of its central proposals are refuted. Read it for the argument, not the
architecture.)

**What (now):** the *surgery* layer for the structure S6 diagnoses. Objects **only make selections**
— `select_sentence`, `select_section`, and the expand/shrink ladder promoted from its hardcoded
4-array to data — and the **existing** operators act on the selection, which is already the shipped
A14 convention (`scope_or_word` = "the selection if non-empty, else the word at the caret"). That
collapses the object × operator matrix into **N + M commands, not N × M**: no palette explosion, no
~150-site `Handler` signature change, and **no command-surface law-10 amendment**. The cross-product
is recoverable in userspace — an Effort-P Lua plugin can bind one gesture to `select_sentence` then
`cut`, which is exactly where law 10's own forward-pointer says parameterized commands belong.

Also in scope: `transpose_sentences` (generalizing the shipped neighbour idiom — `transpose_words`
swaps the word before the caret with the word at it, *preserving the gap between them*); **one
object-agnostic `swap`** over the `MarkedBlock` + `Selection` pair the editor already has (it doesn't
care what produced the spans, so it serves every object forever); and `count_region` — today only a
*view toggle* exists, there is no count-of-region command at all.

**Cut, and why** (each verified against real code): the `TextObject` trait / `ObjectRegistry` /
`BufferView` / `Affinity` scaffolding (Vim-shaped; A14 shipped ten operators as plain fns in a leaf
module with zero trait machinery — and our plugins are Lua, composing *commands*, not Rust traits);
the `PairedDelimiter` framework (**wrong, not merely unverified** — `BlockTree` discards ALL inline
events so the "authoritative" tree path *cannot exist*, fiction conventionally omits the closing
quote across dialogue paragraphs, and an unmatched `"` makes the scan O(document)); the plain-text
degradation matrix (`nav::paragraph_range_at` already covers it); and **section transpose → S1**
(`outline::sections` yields *nested, overlapping* ranges — the "next section" after an H2 is usually
its own H3 child, so naive swap-with-next **corrupts the document**).

**Blocked on S5** — everything here stands on a sentence detector that is currently wrong.

**Status: TRIAGE — captured from an external design-space draft, not yet brainstormed.** The
substance lives in **`docs/design/prose-text-objects-design-space.md`** (a full pre-spec exploration
drafted with an LLM that lacked codebase access — plausible and well-aligned but *unverified*; its
§8 is explicitly open questions for the implementer because it could not read `repar`). That doc is
idea material and a strong starting map; it must go through our grounding-first + brainstorm + Fable
pipeline before any spec, re-deriving its concrete types/seams from the real source.

**Magnitude / theme:** **XL** — an editing-model layer, closer to Effort-P scale than to a single S
item. Filed under Theme S (manuscript structure) for now, but flagged to **possibly promote to its
own theme** if it grows a cluster of items. Relationships: **S1** (rearrangeable outline — the
`Section` object here is the structural primitive that heading-subtree move needs); **C2/C2b**
(`repar` reflow/unwrap/ventilate — *already shipped*; the design-space §8 is entirely about the seam
between the object layer and `repar`, incl. single-sourcing sentence boundaries between `ventilate`
and this module's detector); **A14** (Emacs-parity transpose / word-case — *already shipped*;
overlaps the operator layer, so part of the surface already exists).

**Fit with our model (noted, for the brainstorm):** the draft correctly assumes we are **non-modal
with explicit marks** — so objects are primarily *selection-makers* (mark the current sentence /
section, extend to the enclosing clause), and `repar`-backed operators act on an existing mark. This
is CORE (structural editing on the data-integrity path — edits must flow through
`submit_transaction`/`ChangeSet`), though later intelligence (POS/dependency backend for
clause/phrase) slots in behind the same trait without touching operators — a natural Effort-P/plugin
seam.

**Open questions for the human (triage):** (1) confirm Theme S vs. its own theme; (2) whether this is
a pre-1.0 core capability or a post-Effort-P direction; (3) relative priority vs. S1 (they share the
`Section` primitive — S1 could land first as a subset, or S4 could subsume it).

### S5 — Sentence authority — fix select_sentence, differential suite, sentence motions
<!-- item: S5 -->

**The foundation of the S5 → S6 → S4 → S7 → S8 arc** (`docs/design/prose-structure-arc.md`). Ships
alone; everything downstream stands on it.

**The live bug.** Verified by probe 2026-07-12 against the real crate:

```
textobj::sentence_bounds("Dr. Smith arrived. He was late.", 0)  ->  (0, 4)  ==  "Dr. "
```

`select_sentence` is **wrong today**. Our detector is UAX-29 segmentation, which handles `3.14`,
`10 a.m.`, and `P.I.` correctly but splits after a **title abbreviation followed by a capital** —
`Dr.` / `Mr.` / `Mrs.` / `St.`, the most common abbreviation class in real prose. Meanwhile repar's
`ventilate` has an abbreviation stop-list and gets it right, so **the shipped product already
contains two authorities that disagree about where a sentence ends.**

**Fix:** an abbreviation-aware post-pass over the UAX-29 boundaries (merge a boundary when the
trailing word is in a curated stop set — pure, ~50 lines, testable). ⚠ The design-space doc's
`STARTER_ABBREVIATIONS` is **not shippable as written**: `St` and `Dr` are duplicated, and `No`,
`Co`, `Mon` would eat *real* sentence boundaries. Curate against the fixture corpus.

**The differential fixture suite** — the honest, testable form of "coherence". Assert that our
detector and `run_transform(Ventilate)` agree across a corpus. This is required because **full
unification is impossible**: repar's own `sentence.rs` says `checkcapital`/`checkcurious` are shared
between ventilate *and* the reflow `guess_merge` path, and the reflow path is **frozen by a
byte-exact par-1.53.0 oracle** — the abbreviation stop-list is deliberately quarantined in ventilate
to protect it. So there are **three** sentence authorities and they cannot be merged; they can only
be *pinned* by a test. (This is also why we do **not** absorb repar — see the arc doc, D1/D2.)

**Sentence motions — which BOTH design documents forgot.** `Dir` has Word and Paragraph motions but
**no Sentence**: there is no Emacs `M-a`/`M-e` parity. Cheaper than any operator, likely more used,
and trivially bounded to the paragraph window.

⚠ **Trap for every consumer:** UAX sentence spans **include trailing whitespace** (unlike word
bounds — `"One two. "` → `(0,9)`). Trim to content or gaps double on transpose.

### S6 — Ventilate-as-a-lens — non-destructive sentence view + rhythm gutter
<!-- item: S6 -->

⭐ **The item where the whole arc's thesis is proven or killed.** See
`docs/design/prose-structure-arc.md` for the north star this tests.

**The reframe it embodies.** The original S4 idea sold *selection, movement, transformation*. That is
the weak half — nobody wakes up wanting to select a clause. The strong half:

> **A prose editor that understands sentences can SHOW the writer the skeleton of their prose —
> non-destructively, on demand — and then let them operate on the bones it revealed.**

Diagnosis, then surgery. Writing is revision; drafting is hours and revision is weeks. Every tool a
writer owns serves drafting (focus mode, typewriter scroll) or filing (corkboards). *Revision* —
**this sentence is 41 words and drowns the reader; four of these six open with "The"** — is served by
nothing. (Cautionary case: iA Writer shipped POS-driven "Syntax Control" — their most-demoed, least-
used feature. Structure-as-*selection* may be a programmer's fantasy about how writers work. This
item finds out, cheaply, before we spend a dependency on it.)

**What:** `ventilate` today (`registry.rs` → `transform.rs`) **destructively rewrites** the buffer.
S6 adds it as a **view**: buffer untouched, the *display* breaks one sentence per line; toggle off and
the prose returns **byte-identical**. `RenderMode` already cycles four states — this is a fifth.
Alongside it, a **rhythm gutter** — per sentence, its word count and opening word:

```
 8  The  The committee met on Tuesday.
12  The  The chair, who had prepared, spoke first.
41  The  The proposal, which had been circulating since…
 7  She  She left.
```

The writer sees the 41-word monster and the three `The`s **instantly**. That is the whole
Hemingway-app value proposition, inside a real editor, over a real manuscript, with no cloud and no
rewriting. Plus repeated-opener highlighting within a paragraph — note the codebase already carries
the anaphora corpus (`transform.rs` tests "We will fight on the beaches / …landing grounds / …fields"),
and repar's `prose-prefix` fixup exists *because* anaphora is real.

**Cost: nearly nothing.** Zero NLP. No new objects. No command matrix. No contract amendment.

**Architectural constraint — why S6 must precede S4.** The lens MUST render using **our own detector**
(S5), never repar's. Then the sentence you *see* and the sentence you *select* are the same object
**by construction**, and the two-authority problem stops being a UX hazard. It also *cannot* call
repar even if we wanted to: repar is `&str` → `String`, so extracting boundaries would mean running
`--ventilate` and diffing lines — a whole-document round-trip per render, an outright violation of the
`O(visible) + O(edited)` rule.

**Note this does NOT remove or replace repar.** `reflow` / `unwrap` / `ventilate` stay exactly as they
are — real commands, repar-backed, still the destructive/export path. The lens is *additive*.

**⛔ FAILURE SIGNAL — the cheapest falsification available to us:** the author uses the lens on real
prose for two weeks **and turns it off**. If that happens, **STOP THE ARC.** S7 and S8 are more
expensive bets on the same premise, and the premise will have been disproved for free.

### S7 — Linguistic substrate — harper-brill POS tagger + NP chunker in-process
<!-- item: S7 -->

**The in-process linguistic layer that S8 needs.** Adoption of `harper-brill` was **decided
2026-07-12** on measured evidence (arc doc, D4).

**The finding that made it possible, and it is in neither design document:** the POS backend is
**not** `harper-core`, and it does **not** arrive via the prose-linters effort. `harper-brill` 2.5.0
has exactly **two** direct dependencies (`harper-pos-utils`, `serde_json`) and exposes a **rule-based
Brill POS tagger** (`tag_sentence -> Vec<Option<UPOS>>`) *and* a **noun-phrase chunker**
(`chunk_sentence`) — the design-space doc's `Phrase` object, which it deferred as "needs real
grammatical parsing," is a shipped crate function.

**Measured on this machine 2026-07-12 (a probe crate, not an estimate):**

| | harper-core (H2, ejected 2026-07-11) | **harper-brill (adopted)** |
|---|---|---|
| crates added | +389 | **+119 activated** |
| binary delta | +16 MB (3×) | **+0.95 MB** |
| GPU/tensor backends compiled | cubecl + CUDA + ROCm + wgpu | **none** — `burn` core + `ndarray` only |
| native / FFI (`-sys`) crates | — | **zero** |

⚠ **Do not re-panic on the lockfile.** It lists **491** crates including `burn-cuda`, `cubecl-hip`,
`burn-rocm`, `cudarc` — those are *optional deps that never compile*. `default-features = false`
does its job; the activated count is 119.

**Live proof it does what the clause splitter needs:**

```
"The committee met on Tuesday because the chair insisted."
   DET  NOUN  VERB  ADP  PROPN  SCONJ  DET  NOUN
```

`on` → **ADP** (preposition), `because` → **SCONJ** (subordinating conjunction) — precisely the
distinction that disambiguates "for" / "so" / "yet" and turns clause-splitting from a heuristic trap
into a principled, testable rule.

**This PARTIALLY REVERSES H2**, which pushed harper out-of-process specifically to shed `burn`. The
reversal is deliberate and justified by the numbers: a third of the crates, a sixteenth of the
binary, no GPU stack, no FFI. Recorded in H2's archive entry so a future reader does not think we
forgot.

**⚠ MERGE GATE — not yet satisfied:** `cargo deny` / `cargo audit` has **not** been run against the
119 new crates (neither tool was installed on the machine where this was measured). Supply-chain
surface grows ~40% of the lockfile, and H2 rightly noted this **matters more now that Effort P has
opened a plugin attack surface**. This check must pass before S7 merges.

**Shape:** a `wordcartel-core` module producing POS tags + NP chunks over the **caret's block
window**, **cold-path only** (command/lens-triggered — *never* per-keystroke), cached by
`(block_span, document.version)`. Brill tagging is rule application over a trained table:
microseconds per sentence, no tensors at inference.

### S8 — Prose lenses — POS-driven stylistic X-rays; Phrase/Clause select-only
<!-- item: S8 -->

**The genuinely novel half of the arc** — and the payoff for S7's substrate.

**The lenses.** Every adverb dimmed. Every passive construction (`AUX` + participle `VERB`)
underlined. Every nominalization flagged. Composable with S4's objects: *select every sentence
containing a passive.*

**This is what harper-ls fundamentally CANNOT give us**, and the distinction is the point:
`harper-ls` flags **errors** — things that are *wrong*. These are **stylistic X-rays of prose that is
already correct**, which is what revision actually consists of. No amount of LSP-seam design gets
here; the wire carries diagnostics (ranges, messages, code actions), never a parse. (See the arc doc
§5: the linters effort is *independent* of this one — different engine, different process model,
different latency budget, no shared code.)

**The objects it unlocks.** `Phrase` — the chunker's noun phrases, near-free once S7 lands. `Clause`
— POS-informed, and *only* POS-informed: the design-space doc's rule-based splitter ("split at
`, ; : —` and at conjunctions") **collapses on real prose**, because the Oxford comma fires mid-list,
"for" is usually a preposition, "so" an intensifier, "yet" an adverb. With UPOS the rule becomes
principled: a clause-comma is followed by a conjunction + subject NP + finite verb; a list-comma is
not. Ships behind a **measured precision gate** on a hand-labeled corpus of real prose.

**⚖ THE LAW OF THIS ARC — the reason both objects are SELECT-ONLY:**

> **A linguistic analysis may COLOR, and it may SELECT. It may never MUTATE text without a visible
> selection the writer can see and abort.**

Brill is trained on newswire; the chunker on treebanks. On fiction, fragments, dialect, verse, and
dialogue they **will** mistag. A wrong *highlight* is noise the writer ignores. A wrong
*transposition* is a corrupted manuscript, and the writer never trusts the tool again. (Clause
transpose additionally needs **capitalization repair** — swap "I went home, but she stayed" and you
get a lowercase sentence start — which the design-space doc never mentions. That alone is a separate,
later decision.)

**Nothing here is on by default** (arc doc, D7). This is *revision* machinery; if it intrudes on
drafting it becomes the thing writers hate about Word, and it violates the project's top priority.
The E7 precedent governs: the cost lands in the summoned view.

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
