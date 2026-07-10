# UX backlog — niggling issues, grounded facts, design directions

**Origin:** 2026-07-03 triage session. Fourteen user-reported niggles, organized into five
themes, each fact-checked against the real source (anchors below are as of `63f98de` and may
drift). Each item graduates to the standard gated pipeline (brainstorm → spec → Codex/Fable
review → plan → subagent build) when picked up — this document is the durable triage, not a
spec. **The open-questions ledger was resolved with the user on 2026-07-03** — see "Resolved
decisions" at the end; decisions are folded into each item below. **A second niggle batch
landed 2026-07-04** (fact-checked at `bd3b72c`): A6 palette reachability (which ANSWERS A3's
open "why did the palette read as a subset" question), E3 chrome theming coherence, E4
bundled-themes research.

**Status legend:** `settled-design` (direction agreed, ready to spec) · `needs-design`
(direction sketched, forks remain) · `available-today` (config-only, no code) ·
`fact-checked` (behavior pinned) · `dropped` (decided against, trigger recorded).

**Related:** engineering-health / architecture-debt triage (module size, dependency weight,
incremental-parser posture) lives in [`docs/engineering-health.md`](engineering-health.md)
(2026-07-07 whole-app eval) — that is the home for non-UX structural work.

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

## Theme A — command-surface architecture

### A3b. Item-by-item menu-curation pass
<!-- item: A3b -->

Apply the adopted curation principle (see the three-surface contract section) item-by-item across the
~126 registry commands / ~58 menu set: decide per command whether it belongs in the *menu* (by
category — the commands a word-processor user goes looking for) vs *palette-only* (motions,
navigation, internal plumbing, keystroke-native), bringing only the genuine judgment calls back for
approval. Lower-risk polish; rides whenever. Independent of A3 (A3 fixes the contract-*violation*;
A3b is the contract-*application* sweep). The state-in-label display (E2) is already done.

**Concrete question to resolve in this pass (user-reported 2026-07-08):** should **`filter`**
(`Filter…`, currently `MenuCategory::Edit` — `registry.rs:140`) move to the **Format** menu? Format
already holds the text transforms (reflow/unwrap/ventilate, `registry.rs:300+`), and a shell filter is
arguably a text-shaping op. Weigh Edit (buffer mutation, like cut/paste) vs Format (text shaping).

### A8. A menu listing the open documents to switch between
<!-- item: A8 -->

**Idea (user):** a dynamic menu that lists the currently-open documents so you can switch between them
(a "Window" / "Buffers" / "Documents" menu that auto-populates from the open buffers).

**Grounded (may drift):** a buffer SWITCHER already exists — `switch_buffer` "Switch Buffer…" (View,
`registry.rs:296`, opens `open_buffer_switcher`), plus `next_buffer`/`prev_buffer` — but that is an OVERLAY,
not a menu. This item is the MENU form. Menu categories are a FIXED enum `MenuCategory = [File, Edit, Format,
View, Settings, Export]` (`registry.rs:39-42`) with statically-registered commands. A per-buffer menu is a
NEW menu shape: entries generated from the LIVE buffer list at open time, not
from the static registry. Command-surface implications: the registry is the single source of truth, so a
dynamic menu needs either per-buffer commands registered on open, or a menu-population hook the contract
doesn't have yet. Open Qs: naming (Window vs Buffers vs Documents); ordering (MRU vs open order); does it
also appear in the palette; interaction with C4 (close-buffer prompt). Anchors: `registry.rs`
(`MenuCategory`, `MENU_ORDER`), the menu builder (`menu.rs`), the editor's buffer list, existing next/prev-
buffer commands.

### A9. "Set Wrap Column…" → "Wrap Column: <value>" (state-in-label)
<!-- item: A9 -->

**Idea (user):** rename "Set Wrap Column…" to "Wrap Column" and show the CURRENT wrap-column value in the
label, like the other stateful options.

**Grounded (may drift):** `set_wrap_column` is a STATELESS command in the **Settings** menu (`registry.rs:547`,
label "Set Wrap Column…") that opens a minibuffer (`MinibufferKind::WrapColumn`). (Note: you said "View" —
it's actually Settings.) Showing the value means converting it to STATEFUL (`register_stateful` +
`MenuMark::Value(current)`), mirroring `cycle_scrollbar` / `clipboard_provider` (`registry.rs:480-510`). The
"…" convention means "opens a prompt" — decide whether the stateful label keeps that affordance (e.g. "Wrap
Column: 80…") or separates show-vs-set. Command-surface contract: a stateful option needs its state fn + the
shared setter; the minibuffer flow stays. Anchors: `registry.rs:547` (`set_wrap_column`), `registry.rs:899`,
the state-in-label rule.

### A10. A dedicated "Block" menu for the marked-block commands
<!-- item: A10 -->

**Idea (user):** the existing marked-block commands (mark / move / save a block, etc.) may deserve their OWN
menu, separate from Edit — block-level manipulation is a slightly different mental model than character-level
editing. This is purely a menu-ORGANIZATION question about EXISTING commands — NOT new behaviour and NOT
operation scope (scope is A11).

**Grounded (may drift):** there is already a coherent "marked block" command family (`blocks_marked` module),
today all under **Edit** except one under **File**: `block_begin`/`block_end` ("Set Block Begin/End"),
`mark_block_from_selection`, `block_copy`/`block_move`/`block_delete`, `block_jump_begin`/`block_jump_end`,
`block_toggle_hidden`, `block_clear`, `copy_block_to_scratch`/`move_block_to_scratch`
(`registry.rs:273-290`), plus `block_write` "Write Block to File…" in **File** (`registry.rs:286`). A "marked
block" is a persistent begin/end region distinct from a character selection. A new `MenuCategory::Block` is a
command-surface change: add the enum variant + a `MENU_ORDER` slot, repoint each command's `menu`, keep every
command in the palette (menu ⊆ palette). Open Qs: menu name/position; whether `block_write` also moves (or is
dual-listed); whether the scratch pair belongs here or with scratch; does anything stay in Edit. Anchors:
`registry.rs:273-290` (the block family), `blocks_marked`, `MenuCategory`/`MENU_ORDER` (`registry.rs:39-42`),
the menu⊆palette contract rule.

### A11. Filter + transform SCOPE: whole-buffer vs marked-block vs selection (+ filter docs)
<!-- item: A11 -->

**Questions (user):** (1) does `Filter` operate on the whole buffer, or can it be scoped to a block/selection?
(2) should the transforms (Reflow/Unwrap/Ventilate) operate on a block? (3) settle a STANDARD "block vs
selection" scope so every scope-taking command agrees. (4) does Filter need user-facing DOCUMENTATION /
example filters (and does it work with an arbitrary filter)?

**Grounded (may drift):** `Filter…` (Edit, `registry.rs:140`) opens `MinibufferKind::Filter`; `Transform…`
(View, `registry.rs:185`) and the discrete `Reflow`/`Unwrap`/`Ventilate` (Format, `registry.rs:300-309`) call
`transform::dispatch_transform(..., None, …)` — the `None` is the range/scope arg. **C2 ("Transform scope")
SHIPPED 2026-07-05** decided the TRANSFORM-UNIT rule for Reflow/Unwrap/Ventilate — start there; it likely
answers (2) and constrains (3). The open piece is FILTER's scope + a UNIFIED block-vs-selection convention
shared by filter, transforms, and the marked-block model (A10 / `blocks_marked`): decide whether "scope" =
character selection, the persistent marked block, or the structural block at the caret, and make all
scope-taking commands agree. Also confirm what Filter does today (whole buffer?) and whether it needs
docs/examples. Anchors: `filter` (`registry.rs:140`), `MinibufferKind::Filter`, `transform::dispatch_transform`,
C2 (SHIPPED), `blocks_marked` (marked-block model).

### A12. Scratch buffer = a dedicated TOGGLE, not a cyclable buffer
<!-- item: A12 -->

**Idea (user):** reaching the scratch buffer should be a single TOGGLE command that flips between the current
buffer and the scratch buffer (and back) — bindable to a hotkey. The scratch buffer should NOT be reachable
by cycling (next/prev) or the buffer switcher; it is a special side surface, toggle-only.

**Grounded (may drift):** today scratch is reached ONE-WAY via `goto_scratch` "Go to Scratch Buffer" (View,
`registry.rs:295` → `workspace::goto_scratch`), and it appears to be an ordinary workspace buffer — so it is
ALSO reachable through `next_buffer`/`prev_buffer` (View, `registry.rs:293-294`) and `switch_buffer` "Switch
Buffer…" (`registry.rs:296`, the switcher). The ask: (1) add a `toggle_scratch` command that remembers the
prior buffer and returns to it when invoked from scratch (round-trip), suitable for a hotkey; (2) EXCLUDE the
scratch buffer from the next/prev cycle, the switcher, and the open-documents menu (A8) — making scratch a
dedicated toggle target, not a document in the rotation. Open Qs: keep `goto_scratch` or replace it with the
toggle; what "previous buffer" means if that buffer was closed; one global scratch or per-session. Anchors:
`goto_scratch` / `next_buffer` / `prev_buffer` / `switch_buffer` (`registry.rs:293-296`),
`workspace::goto_scratch`, the workspace buffer list; relates to A8 (switcher — scratch excluded) and the
block→scratch commands (`scratch.rs`).

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

## Working order (recorded 2026-07-04 — dependency-derived, user-approved)

> **Superseded for status/sizing — see the generated `BACKLOG.md` (source of truth `backlog.toml`).**
> Kept below only as the historical dependency-ordering rationale; do NOT read per-item status here.

Hard edges: E3 → E1/E2/E4; D1 carries A5; E2 rides E1; H1 before Effort P. Soft edges: H1
relocates what C4/D1/A5/E1 touch (split early = every later effort lands in focused files);
B2's hanging indent is a wrap-policy feature (travels inside B1); A3's palette parts share
A6's territory; E2's checkable items serve A5/E1; C2 and C3 are islands.

*(Progress: 1 A6 ✓ · 2 H1 ✓ · 3 B1+B2 ✓ · 4 C4 ✓ · 5 C2 ✓ · 6 D1+A5 ✓ · 7 E3+E4 ✓ · **8 E1+E2 ✓**
(2026-07-07 @ f7b7b10 — folded effort: E1 + E2 + overlay/mouse completeness + menu windowing) ·
**B4 SRC-HI ✓** (2026-07-07 @ 1bbd82b) · **C3 clipboard provider-chain ✓** (2026-07-07 @ 16457f9) ·
**R1 typing-latency ✓** (2026-07-07 @ 02ac906) —
**next: eng-health H1 god-object decomposition (Fable-gated — now unblocked), Theme S/P, or A3b
menu-curation; then Effort P**. A3's palette-completeness invariant + item-by-item menu-curation
pass remains open.)*

*(NEW 2026-07-06: **R1 editing-responsiveness** entered brainstorm mid-stream — a
tight-scope perf/correctness fix (Theme R). Dependency-free; recommended to slot BEFORE Effort P.
May preempt or run alongside E1+E2 at the user's call.)*

1. **A6** palette reachability — folds in A3(a) hints-verification + the palette-completeness
   invariant test (same territory). Kills the invisible-dispatch hazard first.
2. **H1** app.rs decomposition — immediately after the small win; everything behind it lands
   in focused files.
3. **B1 + B2** word-boundary wrap + list indent, ONE effort (bullet indent + hanging indent
   inside the wrap work). The user's highest-value rendering fix; the E-arc then gets judged
   on correctly wrapped text.
4. **C4** close-buffer prompt — lands in the post-H1 prompts.rs, reuses the quit machinery.
5. **C2** transform scope — SHIPPED 2026-07-05.
6. **D1 + A5** settings write-back + keymap switch — SHIPPED 2026-07-05.
7. **E3** chrome theming coherence (+ the render.rs split; **E4's research kicks off in
   parallel** — pure reading, no code dependency).
8. **E1 + E2** density presets + polish — **SHIPPED 2026-07-07 @ f7b7b10** (folded effort:
   E1 density presets + E2 polish + overlay/mouse completeness + menu windowing). A3's
   menu-curation pass was NOT folded in — remains open.
9. **C3** SSH/tmux clipboard — **SHIPPED 2026-07-07 @ 16457f9** (full works-everywhere provider
   chain; both final gates GO; smoke 8/8).

Then **Effort P**, landing on a decomposed app.rs, a coherent chrome model, and a settings
rail. FLAGGED JUDGMENT: B1 sits before the D/E arc on value; pure dependency logic permits
swapping blocks 3 and 6-8 if the visual-consistency wins should bank first — B1 is
dependency-free in both directions.

### Pre-Effort-P checklist (must clear before P)

- **repar re-plumb check** — `DONE` 2026-07-06 (merge 10a5a05, pushed). Adopted repar 1.1's
  documented "blessed editor stack" (`none,all,prose,prose-prefix,markdown,no-trailing-pad`); the
  two "upstream candidates" were NOT bugs — repar 1.1 released them as opt-in fixups
  (`no-trailing-pad` for CJK trailing-space→hard-`<br>`, `prose-prefix` for the anaphora trap),
  both proven load-bearing by Fable probes. Migrated `run_transform` onto repar's SemVer-frozen
  `from_par_args` surface; Cargo.lock pinned 1.1.0 (a REAL pin — the tokens require it, not drift).
  Codex + Fable both GO. See [[wordcartel-repar-integration]]. Original checklist retained below
  for history. `repar` is a PATH dependency
  (`repar = { path = "../../par-command/repar" }`, wordcartel/Cargo.toml:12), so it builds
  against whatever the user has locally — the Cargo.lock pin (1.1.0) drifts every build. repar
  has been updated AGAIN; before P, re-examine the integration against the current local repar:
  (1) the ONLY API surface is `transform.rs::run_transform`
  (`repar::Options::new().width(width).map_err(...)`, transform.rs:314 — the builder-returns-
  `Result` shape from repar ≥1.0); verify the API still matches and no new required options were
  added. (2) The wrap_column width knob (repar10) still wires through. (3) Re-check the six
  repar-1.0 contract pins (change.rs / transform tests, backlog C2 area ~:256) — a new repar may
  fix or shift the deliberately-frozen behavior (incl. the CJK trailing-space artifact). (4) The
  TWO upstream repar report candidates (CJK trailing-space→hard-`<br>`; prefix-inference anaphora
  mangling, ~:257) — did the update address them? (5) Decide whether to take a deliberate version
  bump + lock sync (vs the current always-drifting path pin). Isolated + low-risk, but P builds
  a plugin system ON this substrate, so it must be confirmed clean first.

## Sizing summary

> **Superseded — the live open/shipped/sizing rollup is the generated `BACKLOG.md` (source of truth
> `backlog.toml`). The hand-maintained snapshot below is frozen history as of the migration and is no
> longer updated — do not trust it for current status.**

**SHIPPED** (see each item's header for the commit): A1, A2, A3, A5, A6, B1, B2, B3, B4, C1, C2, C3,
C4, D1, E1, E2, E3, E4, H1, R1 — plus the repar re-plumb check. (A4 dropped.) The remaining open work,
by size:

- **Small:** A3b item-by-item menu-curation pass (A3 itself shipped; the state-in-label half
  shipped with E2) · A6-followup overlay mouse parity · A9 Wrap-Column state-in-label · H7 unwrap audit.
- **Small-Medium:** S3 Snapshots (Theme S — capture/restore/persist reuse existing machinery;
  the one net-new algorithm is a display diff) · B6 heading-glyph style toggle.
- **Medium:** eng-health H1 god-object decomposition (`app.rs`/`render.rs` SEAM refactor — Fable-gated,
  now unblocked; gated before Effort P) · S1 rearrangeable outline / corkboard.
- **Larger:** S2 directory-as-binder (post-Effort-P plugin) · Theme P plugin candidates (all
  post-P — goals/streaks, style lens, CMS publish, backlinks, CriticMarkup/Fountain/wikilinks).
- **`needs-design`, sizing TBD:** A8 open-docs menu · A10 Block menu · A11 filter/transform scope ·
  A12 scratch toggle · B7 selected-item legibility (`potential-bug`) · B8 caret shape/color · E6 splash ·
  eng-health H2 dep-weight · H5 swap-file cleanup · H6 version scheme.

Then **Effort P** (the in-process Lua plugin system) — the 1.0 capstone.

## Newly-tracked items (stubs)

*(Auto-created during the backlog-manifest migration. Status/size/kind live in `backlog.toml`; flesh out the triage prose here when the item is picked up.)*

### A13 — Overlay mouse parity
<!-- item: A13 -->

Click-to-select for theme picker + file browser; outline click-to-jump.

### P — Effort P — in-process Lua plugin system (1.0 capstone)
<!-- item: P -->

The plugin/automation spine; registers into the command/hook/job seams. See docs/design/effort-p-plugin-system-design-space.md.
