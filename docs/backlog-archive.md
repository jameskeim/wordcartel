# Backlog archive — completed & dropped items

Triage prose for **terminal** backlog items (shipped / dropped). Status, commit, and date
are the manifest's (`backlog.toml`) — see the generated `BACKLOG.md` for the rollup. This
file is the detailed history; it is kept marker-linked so the drift gate covers every item.

## Theme A — command surface

### A1 — Menu bar states + mouse reveal
<!-- item: A1 -->

**Facts:** F10 is the only activation (`keymap.rs:243`); the menu opens straight to
pulled-down (first dropdown visible — `menu::empty()` + hydrate, `app.rs:824-826`); there is
no bar-visible-inactive state (bar renders only while `editor.menu.is_some()`,
`render.rs:906`); no visibility config key. **Mouse plumbing already complete** once a bar
exists: click bar label switches category, click dropdown row dispatches, click-away closes
(`mouse.rs:115-142`, hit-testing shared with the renderer).

**Settled design:**

```toml
[menu]
bar = "hidden" | "auto" | "pinned"   # default: auto (CONFIRMED 2026-07-03)
```

- `hidden` — today's behavior; F10/palette are the doors.
- `auto` — bar hidden; pointer RESTING on row 0 for a dwell (~250 ms, tunable) reveals it;
  click the revealed bar to open; pointer-leave / menu-close hides after a grace period.
- `pinned` — bar always visible-inactive; F10 or click pulls down; **Esc closes the dropdown
  back to visible-inactive, not to hidden** (close-menu ≠ hide-bar in pinned mode).
- **F10 remains the pulldown keystroke in every mode** (its meaning narrows from "toggle the
  apparatus" to "open/close the dropdown"). Architecturally: split the conflated
  `editor.menu: Option<MenuView>` into two orthogonal bits — *bar pinned/shown* (chrome
  state) × *menu open* (overlay state). No bar-focused-without-dropdown state (adds a state,
  adds no value given click + F10).
- **Optional garnish (decided as optional, not committed):** within-menu letter mnemonics —
  while a menu is open, plain letter keys jump to items. Zero global keymap footprint. May
  ride along if cheap; see A4.

**Dwell safeguards:** transit ≠ dwell; never arms during a drag (button held = selecting);
wheel events don't count; a click on row 0 while hidden is a TEXT click (reveal comes only
from dwell or F10); if mouse capture is off or the terminal lacks motion reporting, `auto`
degrades silently to `hidden` + F10.

**Implementation notes:** the scrollbar already does transient chrome (auto-show on activity,
self-hide 1200 ms — `recompute_scrollbar_visible`); the reveal timer is the same pattern with
a dwell trigger, slotting into the run loop's existing `recv_timeout` deadline array. Motion
events already flow through the mouse path (currently ignored). Dwell duration is an
implementation tunable, not a design fork.

### A2 — Full-width menu bar fill
<!-- item: A2 -->

**Facts:** only the label rects get the Chrome style (`render.rs:108-118`, `:915-920`); gaps
and the right side are unstyled — no full-width fill.

**Decided (2026-07-03):** fill row 0 with the bar background at all times the bar is shown —
**background only; no right-edge content by default.** Right-edge content (leading candidate:
buffer name + dirty marker) is designed once, deliberately, inside E1's full-chrome work — not
defaulted piecemeal now. Labels truncate before any future content on narrow terminals.

### A3 — Option reachability + preset-aware hints
<!-- item: A3 -->

**SHIPPED 2026-07-07** (merge `d7a5494`, branch effort-a3-option-reachability). Delivered: shared
`Editor` setters (`set_scrollbar_mode`/`set_status_line_mode`/`set_menu_bar_mode`) with `apply_bundle`,
`menu_bar_pin`, the new commands, AND app.rs startup all routed through them (law 6); the 10
option-reachability commands (scrollbar/status_line/menu_bar set-per-state palette-only +
`cycle_scrollbar`/`toggle_status_line` View representatives — closing the ZEN/FULL law-2 orphans);
`chord_for` prefers a user's config-patch binding over the shortest preset default (law 7,
`KeyTrie.user_bound`); the three law regression tests (recurrence guard with a compile-time
`SettingsSnapshot` field-binding, palette-completeness, preset-aware hints); a contract shape-rule-8
refinement (menu representative = toggle OR cycle). **Menu chord right-justification (folded in as
Part 4, user decision)** also shipped — dropdown chords now align flush-right like the palette.
Gates: full suite green (888 + 278 + integration), clippy clean, smoke 8/8; Codex spec+plan+pre-merge
GO (pre-merge caught a real startup law-6 bypass + a missing recurrence-guard binding); Fable
whole-branch PASS (14 execution probes). Residual polish — RESOLVED in a follow-up pass 2026-07-07: (fixed) `toggle_status_line`
catch-all now exhaustive (`Auto | Off => "Auto"`); `user_bound` import tidied (`HashSet` via the
`use`); the setter test made symmetric (both dwell timers + the mode asserted per surface). (Kept by
design) the palette-completeness count-assert stays — it guards against duplicate/extra rows and keeps
the law test self-contained; the `chars().count()` measure in `right_justify_leaves`/`menu_dropdown_rect`
matches the codebase's existing menu-measurement convention (menu labels are English command names, so
CJK display-width is theoretical) — changing only the new code would break alignment consistency, so
it's left as a codebase-wide note, not a per-effort fix. The original settled-design notes below are
retained for history.

**Design ratified 2026-07-07** (brainstorm). Refocused from the original "palette follow-ups +
menu item pass": the ZEN/FULL density decision (E1) added options — `status_line` mode and
`scrollbar` mode — that have NO individual command, so they're reachable ONLY via the `toggle_chrome`
profile or by hand-editing config. That VIOLATES the three-surface contract (palette is exhaustive;
deviations are bugs) — and, because the command registry is the spine of the future Lua plugin
system (Effort P), a command-less option is also **plugin-uncontrollable** (a plugin mutates state by
dispatching commands). A3 closes that gap and locks the hint plumbing. **The broad item-by-item
menu-curation pass split out → A3b.**

**Decided:**
1. **Option-reachability — the keymap-pattern shape.** Every preset-owned option gets explicit
   **set-per-state** commands (deterministic — a plugin/script needs "set to X," not "cycle and
   hope"; tagged `menu: None` → palette-only, like `keymap_cua`/`keymap_wordstar`) **plus a cycle**
   command (in the menu, state-in-label — the human convenience, like `keymap_next`). New commands:
   `scrollbar` (Off/Auto/On), `status_line` (Auto/On — no true Off), `menu_bar` (Hidden/Auto/Pinned,
   a real 3-way alongside the existing `menu_bar_pin`). **Set-handlers and `apply_bundle` call ONE
   shared setter per option** (e.g. `set_scrollbar_mode(editor, mode)`) so the profile and the
   commands cannot drift — the profile becomes "a batch of the same setters a plugin could call."
2. **Recurrence guard test:** assert every persisted setting (each `SettingsSnapshot` field / config
   key) is changeable through *some* command / command-surface — so a future option can't ship
   command-less.
3. **Hint display policy:** `chord_for` (`keymap.rs:180`) must **prefer the user's explicit
   (patch-bound) binding over the shortest inherited default** (today it returns shortest-then-
   alphabetical, which mis-shows a command that has an added custom binding beside its default).
4. **Hints-verification tests:** pin that (a) menu + palette hints re-resolve after a CUA↔WordStar
   switch (A5 wired the trie rebuild; UNTESTED), and (b) a custom bind surfaces in BOTH surfaces.
5. **Palette-completeness invariant test** ("every non-hidden registry command appears in the
   palette") — formalize the near-miss at `palette.rs:138`.
6. **Plugin-forward posture:** commands stay NULLARY now; parameterized commands (`set_scrollbar("off")`)
   are deferred to Effort P (which will define the plugin API) — but keep the set-value semantics clean
   so P can later collapse the N explicit-set commands into one parameterized command without breaking
   the contract. See [[wordcartel-plugin-roadmap]].

Grounding (already TRUE, retained): both surfaces show active-keymap chords via `chord_for` (menu
bakes it into `leaf_label`; palette sets `row.chord`); A5 rebuilds the trie on preset switch; custom
`[keymap]`/`[keymap.cua]`/`[keymap.wordstar]` patches fold into the trie via `build_keymap` and so
into both surfaces (scoped patches are preset-aware). Palette is exhaustive in DATA; the MENU is the
curated subset (A3b). A6 fixed the palette-reach ("only a subset" impression).

### A4 — Menu accelerators (Alt+F/Alt+E)
<!-- item: A4 -->

**Decided: no global Alt accelerators.** With the settled A1 model, any category is two
keystrokes away (F10 + arrows) or one dwell+click; the menu is a *discovery* surface — speed
users graduate to bindings and the palette. Global Alt+letter bindings cost real conflict
surface (Alt+Z is fold; `Edit`/`Export` collide on E; every preset inherits the reservations)
for a layer nobody has asked to use. **Revisit trigger:** actual user demand. The low-conflict
middle path — within-menu mnemonics while a menu is open — is recorded as an optional A1
garnish (see A1), not a commitment.

### A5 — Switch keymap preset from the menu
<!-- item: A5 -->

`keymap_cua`/`keymap_wordstar` commands (new **Settings** menu category, palette-searchable);
the trie rebuilds between reduces via `rebuild_keymap_if_requested` (one source of truth used
by run(), the e2e Harness, and the seam test); a half-typed chord prefix never completes
against the new base; the switch status survives the rebuild; hints stay fresh (palette
recomputes, menu rebuilds per open). Preset-scoped patches (`[keymap.cua]`/`[keymap.wordstar]`)
ride on top of every base — "later file wins; within a file, specific wins"; `keymap::PRESETS`
is the single preset list the pins iterate. **WordStar now binds `f10 → menu`** — the live
sanity caught that the preset bound NO command surface, making runtime switching a keyboard
trap (palette/menu were CUA-only; no switch-back without a mouse). Persistence rides D1.
E2's radio marks get `editor.active_keymap_preset` as their hook. C4 closure recorded:
`close_buffer` stays unbound by design in both presets (pinned); scoped patches are the
supported user binding path.

**Original facts:** `build_keymap` runs exactly once at startup (`app.rs:2029`); no runtime rebuild
path; no switch command; presets = cua, wordstar. **Direction:** a `keymap_preset` command
(menu: View or a Settings home) → rebuild the trie between reduces in `run()` (flag/Msg-driven;
the trie is borrowed by `reduce`); menu hints stay fresh automatically (menu rebuilds on every
open); palette hints must re-resolve. Persistence rides on D1 — these two ship together.
Checkable/radio menu items (E2) show the active preset.

### A6 — Palette/overlay full-list scrolling + wheel
<!-- item: A6 -->

*(Added 2026-07-04; answers A3(b). Facts as of `bd3b72c`.)*

**Shipped:** `scroll_top` added to all four overlays (Palette, OutlineOverlay, ThemePicker,
FileBrowser); shared `list_h_for` / `keep_visible` module (`list_window.rs`); render painters
self-heal on every frame (resize-safe); key arms re-window after every selection change; mouse
wheel arms for palette/theme-picker/file-browser (tp wheel also previews correct row); PgUp/PgDn/
Home/End for all four overlays; scrolled-descend reset (panic-class C1 fixed); `windowed_indicator`
helper in render.rs replaces inline copies across all four painters.

**Remaining follow-up (overlay mouse parity) — `needs-design` · Small:** click-to-select for
theme picker and file browser list rows (the same `palette_row_at` pattern, not yet wired); an
outline mouse block (currently the outline overlay swallows ALL mouse events — no click-to-jump);
A3 curation pass (reduced — the "subset impression" was reachability, now fixed). Promote to a
full effort when prioritized.

### A7 — Right-justify the state value in stateful menu rows
<!-- item: A7 -->

**Observation (user):** in the Settings menu, stateful rows like `Clipboard: Auto` and `Keymap: CUA`
should have the value (`Auto`, `CUA`) **right-justified**, not run inline after the colon.

**Grounded (may drift):** stateful leaves render today as one left-aligned string `"{base}: {value}"`
(`menu.rs:~57`, `leaf_text`, from `MenuMark::Value` / `registry.rs:47`). The dropdown ALREADY
right-justifies a different field — the chord hint — to a common flush-right column (`right_justify`,
`menu.rs:~74`, matching the palette). So the ask is to extend that flush-right treatment to the state
value: render `Clipboard        Auto   <chord>` (base left, value in a right-aligned column) instead of
`Clipboard: Auto   <chord>`. Direction: split stateful leaves into base + value and align the value
column, coexisting with the existing chord column and leaving stateless rows unchanged. Anchors:
`menu.rs` (`leaf_text` state-in-label + `right_justify`), `MenuMark::Value` (`registry.rs:47`).

### A3b. Item-by-item menu-curation pass
<!-- item: A3b -->

Apply the adopted curation principle (see the three-surface contract section) item-by-item across the
~156 registry commands / ~72 menu set (Phase-0 count, 2026-07-10): decide per command whether it belongs in the *menu* (by
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

### A13 — Overlay mouse parity
<!-- item: A13 -->

**Corrected grounding (Phase-0 map, 2026-07-10):** the overlays originally named here — theme
picker, file browser, outline — ALREADY have scroll + click-to-select/jump wired in
`mouse.rs::route_overlay`; they need no work. The real mouse-parity gap is the **minibuffer** and
**search** overlays, which have ZERO mouse handling today (`route_overlay` drops their events). A13
= add click-to-position-cursor in the minibuffer + click-to-jump-to-match in search, keeping the
keyboard path authoritative (contract law 5 — every mouse affordance has a keyboard path). Seam:
`mouse.rs::route_overlay` (add the `minibuffer`/`search` branches).

### A14 — Emacs-parity prose editing commands (transpose, word-case, join-line, whitespace fixups)
<!-- item: A14 -->

A cluster of atomic editing commands that Emacs ships built-in, classic WordStar lacked, and Wordcartel's
command registry does **not** have today (verified against the registry — `commands.rs` / `registry.rs`):

- **Transpose** — swap the two chars around the caret / the two words around it / two adjacent lines (Emacs
  `transpose-chars`/`-words`/`-lines`, C-t / M-t / C-x C-t). The classic typo-fixer; nothing equivalent exists.
- **Word/region case** — upcase / downcase / capitalize the current word or the selection (Emacs M-u / M-l /
  M-c + `upcase-region` etc.). No case command in the registry today.
- **join-line / delete-indentation** — pull the next line onto this one, collapsing the join to one space
  (Emacs M-^). Absent.
- **Whitespace fixups** — `just-one-space` (M-SPC), `delete-blank-lines` (C-x C-o), optionally
  `delete-horizontal-space` (M-\). Absent.

**Scope discipline (what is NOT here):** keyboard macros and dynamic-abbrev (dabbrev) are *automation*, not
single commands — that is **Effort-P** territory (record/replay over the registry), tracked under P, not here.
`sort-lines` is already reachable via the `filter` pipe; incremental search (`find`), query-replace
(`replace`), and paragraph reflow (`reflow`) already exist; code-editor motions (sexp/list, comment-region,
narrowing) are out of domain for a markdown prose editor.

**FOLD INTO THE COMMAND-SURFACE CURATION EFFORT** (with A3b / A8 / A9 / A10 / A11 / A12 / A13): every new
command lands on the same surface those items touch — the name-keyed registry, the exhaustive palette, and the
command-surface contract — so do them as one effort rather than re-loading that context per item. Each command
is **keymap-agnostic**: it enters the registry once and is bound as fits in the CUA / WordStar presets (and
placed in Edit ⊆ palette). Follow the existing atomic-edit pattern (`delete_word_back` / `delete_word_forward`
/ `delete_to_line_end`, which live in **`commands/edit.rs`**): compute the range, build a `ChangeSet` + `Edit`,
and call **`editor.apply(txn, edit, EditKind::Other, clock)` directly** + `settle_after_edit` — undo/marks stay
correct through `editor.apply`. **Corrected grounding (Phase-0 map, 2026-07-10):** do NOT route these through
`submit_transaction` — that is the M2 *untrusted*-edit boundary (the Effort-P `apply(Transaction)` seam for
plugin/automation input), a separate path from internal built-in commands. Likely **S–SM** once scoped — each command is small; the count is the work.

Anchors: the command registry (`registry.rs`), the existing word/line delete commands in `commands/edit.rs` as
the pattern (`editor.apply` + `settle_after_edit`), and `docs/design/command-surface-contract.md`.

## Theme B — rendering

### B1 — Word-boundary wrap (UAX #14)
<!-- item: B1 -->

**Shipped:** UAX #14 word-boundary soft-wrap (`unicode-linebreak` 0.1.5, wordcartel-core
only): breaks computed on the VISIBLE grapheme sequence (mid-cluster offsets dropped);
tail re-placement under a repeating overflow guard with grapheme fallback; trailing
whitespace hangs at the edge (render caret clamps to the last text column); **fenced code
blocks keep grapheme wrap byte-identical** (role exception, user decision — no config key).
Laws: 3 amended composably, new W1/W2; strategy gained a bare combining-mark token.
**Known accepted wart:** the crate implements Unicode 15.0 (pre-LB20a) — a word-initial
hyphen (`-flag`) may wrap after the `-`; upstream is maintenance-only, revisit if it bites.

*(Original facts/direction below are historical.)*

**Facts (historical):**

the soft-wrap was greedy PER-GRAPHEME (`layout.rs:261-292`): when
`col + vg.width > vw` the overflowing grapheme moves to the next row — no word-boundary
lookback/lookahead of any kind. Words break mid-word at the viewport edge.

**Direction:** break at whitespace with the standard overflow fallback (a single word longer
than the line still breaks). Touches the per-frame hot path (`layout()`) and ripples into
`ColMap`/caret/click mapping (the shelved-F8 territory — word-wrap does NOT change the
bound-to-visible-rows rejection, it changes row break positions only). Should travel with
hanging indent (B2's companion). CJK/no-space text falls back to grapheme wrap. Pin with e2e
Harness journeys (wrap + caret round-trip).

### B2 — Sub-list bullet indent + hanging indent
<!-- item: B2 -->

**Shipped:** the ListItem indent scan is tab-aware (spaces AND tabs) and MARKER-CONDITIONAL
(continuation lines keep visible indent, no glyph); the indent conceals into the prefix
glyph (`"  • "`, `"   2. "`, tab = 4 cols) so bullets paint at their indent level and
wrapped items hang under their TEXT via the existing prefix-width reset — zero render
changes. Blockquotes deliberately untouched. Bullet columns mirror SOURCE indent
(CommonMark treats ≤3 spaces as the same level — deliberate source-faithfulness).

*(Original facts below are historical.)*

**Facts:** for `"  - sub"`, `apply_block_prefix_conceal` (`md_parse.rs:252-289`) conceals the
marker + its space but the LEADING SPACES SURVIVE as visible graphemes, while the `"• "`
prefix glyph is always painted at column 0 with `prefix_width = 2`. Rendered: `•   sub` — the
nested bullet sits at the SAME column as the parent's, text pushed right. Exactly the reported
"text indents, bullet doesn't."

**Fix shape:** conceal the leading indent too and emit the prefix as *indent + bullet*
(`"  • "`) so the bullet lands at its indent level (generic for deeper nesting). Verify
ordered-list markers get the same treatment. **Companion:** hanging indent — wrapped
continuation lines of a list item align under the item's text, not the bullet (interacts with
B1; consider shipping together).

### B3 — Heading glyphs default ON in every theme
<!-- item: B3 -->

**Facts:** `Theme.heading_level_glyph: bool` (`theme.rs:119`); shade ramp `█ ▓ ▒ ░ ▏ ·`
H1→H6 (`render.rs:16-18`, gate at `:412-421`). ON for `no_color` + the phosphor-*flat*
variants; OFF for default/tokyo-night/base16. A config key already exists and works with any
colored theme TODAY:

```toml
[theme]
heading_level_glyph = true
```

**Decided (2026-07-03): ALL themes default `heading_level_glyph = true`** — the shade ramp
becomes part of wordcartel's visual identity everywhere, rendered in each theme's heading
color; `heading_level_glyph = false` remains the one-line opt-out. One line per theme plus an
eyeball pass (colored themes already style headings; the glyph is a deliberate second signal).

### B4 — SRC-HI per-construct syntax highlighting
<!-- item: B4 -->

**SHIPPED** as `effort-srchi-highlight` (merged `--no-ff` @ 1bbd82b, pushed). SRC-HI now shows raw
markdown with every construct — inline delimiters, block prefixes, content — colored in the current
theme's element faces (uniform per-construct, no new faces); SourcePlain stays monochrome,
LivePreview untouched. `LineRender {Concealed,RawPlain,RawStyled}` replaced the `is_active` bool;
`analyze` RawStyled branch styles whole spans (delimiters included); `layout` last-match (`.rfind`);
`LayoutKey` mode-aware; render `plain_source` gate in both paint paths. Geometry ≡ SourcePlain
(no cursor change). Both final gates GO (Codex pre-merge + Fable 9/9 execution probes). Follow-ups
recorded in the effort ledger (all follow-up-ok): RawStyled Code/comment/link core test, SP-side
base_fg coverage, stale nav doc comments. The original triage below is retained for history.

**Symptom (user-reported):** cycling to the `SRC-HI` render mode looks the same as `SOURCE` —
no syntax highlighting of the raw markdown.

**Root cause (fact-checked 2026-07-07):** `RenderMode` has three variants (`editor.rs:45-48`:
`LivePreview`/`SourceHighlighted`/`SourcePlain`) and the cycle/status/label all treat them as
three (`commands.rs:482-484` cycles LP→SH→SP; `render.rs:348-350` labels PREVIEW/SRC-HI/SOURCE).
BUT every place rendering actually decides behaviour reduces the enum to a **binary**
`source_mode = view.mode != LivePreview` — `derive.rs:222`, `render.rs:607`, `nav.rs:64`. So
`SourceHighlighted` and `SourcePlain` take the IDENTICAL path (raw markers, no
concealment) and there is no branch that applies token/syntax coloring to distinguish SH. SH is
effectively dead — a labelled third mode that renders as SOURCE. (A test at `commands.rs:1212`
only pins "SH shows raw markers," which SP also does, so it never caught the collapse.)

**Deeper root cause (styling engine):** the collapse bottoms out in `md_parse::analyze(line,
role, is_active)` (`md_parse.rs:11`), which short-circuits when `is_active` is true —
`if is_active || line.is_empty()` returns the full raw source as one run with `styles: vec![]`
(`md_parse.rs:8-16`). `source_mode` forces `is_active_effective = true` for every line
(`derive.rs:264`), so both SH and SP get raw text with NO styles. There is no "raw + styled"
path today.

**Intended distinction:** LivePreview = concealed markers + theme colors (WYSIWYG-ish);
**SRC-HI = raw markers VISIBLE *plus* markdown tokens colored** (code-editor-style syntax
highlight); SOURCE = raw markers, no color (monochrome plain).

**Sizing: SMALL and contained** (fact-checked 2026-07-07) — two de-riskers:
1. **The color data already exists independent of concealment.** In `analyze`'s non-active path,
   `conceal: Vec<Range>` and `styles: Vec<StyleSpan>` are *separate* lists (`md_parse.rs:26-141`;
   styles carry source-byte ranges). "Raw + colored" = apply the `styles` spans *without*
   applying the conceal grid — the data is already computed; nothing new to parse.
2. **SRC-HI geometry ≡ SourcePlain.** SH conceals nothing (like SOURCE), so its cursor/wrap/
   fold/ColMap math is IDENTICAL to SP — color is a pure visual overlay on the same grid. The
   entire geometry layer (`visible_to_source`, cursor stops, soft-wrap, the ColMap) is UNTOUCHED
   (that's the part that would make it Medium+; it's off the table).

**Fix steps:** (a) replace `is_active: bool` with a 3-way mode (conceal+color / raw+color /
raw+plain) threaded through `analyze` → `layout::layout` → `visible_width`/`visible_source` →
`derive.rs`; (b) add the "styled-but-not-concealed" branch in `analyze` (run the parse, keep
`styles`, skip conceal) — the ONLY real logic change; (c) **fix the layout cache key** —
`LayoutKey` carries `source_mode: bool` (`derive.rs:242`), so SH and SP currently share a cached
layout (a second manifestation of the same bug) — the key needs the real mode or SH/SP won't
differ even after the render fix; (d) add a real **SH ≠ SP regression test** (the missing
coverage that hid this — `commands.rs:1212` only pins "SH shows raw markers," which SP also does).

**Theming (directive, 2026-07-07): SRC-HI uses the CURRENT THEME — automatically.** The `styles`
spans are `Style` enum values composed against the active theme's faces at paint time (the same
faces LivePreview paints), so SRC-HI tracks the theme for free — no separate syntax palette.
Sub-choice this surfaces: `styles` cover the CONTENT ranges; the MARKERS (`#`/`**`/backticks) are
in the separate `conceal` list, so the free result is **content themed, markers plain** ("delimiters
dimmed" look). Tinting the markers themselves would need a new punctuation/dim face (none exists
today) — default to plain markers unless a marker face is added.

**Product fork:** keep the LivePreview ACTIVE line plain (current "edit-this-line" look) while
SRC-HI colors every line uniformly — i.e. SRC-HI is its own treatment, not "every line is a
preview-active line." **Alternative (cheaper):** drop SH entirely (2 modes PREVIEW+SOURCE —
delete the variant, fix the cycle at `commands.rs:482` + label) if the highlighted-source view
isn't wanted. **Recommendation: FIX, don't drop** — the styles are already computed and the
geometry is already correct, so a genuine colored-markdown-source view is near-free; one of the
better effort-to-payoff ratios on the list.

---

### B5 — Low heading-glyph collision (H5/H6)
<!-- item: B5 -->

**Observation (user):** the **heading-level glyph** (the B3 prefix marker, not the color) for H5/H6
looks like other constructs' prefixes — **H6's glyph is literally a bullet point** (reads as a list
item) and **H5's glyph is basically the vertical bar drawn in front of a blockquote**.

**Grounded — confirmed, this is a real glyph collision (may drift):** the heading glyphs are a single
shade ramp, `SHADES = ["█", "▓", "▒", "░", "▏", "·"]` (`wordcartel/src/render.rs:18`, used both in
cue/no-color mode and when `heading_level_glyph` is on). The ramp reads as "heading intensity" for
H1–H4 (`█▓▒░`, solid→light blocks), but its low end lands on shapes other prefixes already own:
- **H5 = `▏`** (U+258F LEFT ONE EIGHTH BLOCK) vs the **blockquote prefix `▎`** (U+258E LEFT ONE QUARTER
  BLOCK, `render.rs:~2003`) — *adjacent codepoints*, both thin left-edge vertical bars; near
  indistinguishable at a glance.
- **H6 = `·`** (U+00B7 MIDDLE DOT) vs the **list bullet `•`** (U+2022, `render.rs:~1968`) — both dots;
  H6 reads as a bullet.

**Compounding (secondary):** for H6 the color aliases too — in every `from_base16` theme H6 fg and
`list_marker` fg are the same slot `b[0x8]`, so an H6 is *both* a dot glyph AND the list-marker hue →
doubly list-like. (Color is the smaller issue; the glyph is the reported one.)

**Direction (forks remain):** pick H5/H6 glyphs that stay in the "heading" visual family yet are
distinct from `▎` (blockquote) and `•`/`·` (list) — e.g. rework the ramp's low end so it doesn't
decay into a bar and a dot, or use a different heading-marker idiom for the deep levels. Must hold in
cue/no-color mode (SHADES drives that path too) and reserve the same 2-col prefix width
(`layout.rs:894`). Anchors: `SHADES` (`render.rs:18`), the blockquote glyph `▎` + list bullet `•`
(`render.rs`), B3 (heading glyphs default on).

*(B5 SHIPPED 2026-07-08 in the polish batch — `SHADES` is now the single-axis ramp `█▆▅▄▃▂`
(`render.rs:20`); this entry's "grounded" section shows the pre-fix ramp.)*

## Theme C — document workflow

### C1 — LaTeX export + xelatex PDF + export typography
<!-- item: C1 -->

**Facts:** export is already PANDOC-based and async (`export.rs`; html captures stdout,
docx/pdf via temp+rename, `Msg::ExportDone`, pandoc probed via OnceLock). **`export_pdf`
passes no `--pdf-engine`** → pandoc defaults to pdflatex — wrong for a xelatex user (fontspec/
Unicode documents will mangle or fail). Pandoc's markdown reader enables its `smart` extension
by default, so today's exports almost certainly already emit typographic quotes/dashes —
implicitly (verify in-effort).

**Direction + decisions:** (a) `export_tex` via the same `run_export` machinery
(`pandoc -s -t latex`); (b) config: `export.pdf_engine = "xelatex"` (+ possibly
`export.pdf_variables` for letterpaper/mainfont) — fixes PDF and gives the .tex export its
personality; (c) **Decided (2026-07-03): export-time typography adopted** as
`export.typography = true` (default ON — matches today's implicit behavior, now owned and
documented); `false` appends `-smart` to the pandoc source format for strict literal output.
In-editor source remains untouched — fully compatible with source-as-is.

### C2 — Transform scope (Reflow/Unwrap/Ventilate)
<!-- item: C2 -->

Both decided rules landed, deepened by eight spec-review rounds into the TRANSFORM-UNIT rule:
the caret acts on the nearest ListItem (marker + indent included, deepest for nested), else the
nearest BlockQuote, else the leaf block; blank-line caret → "nothing to transform" (gaps never
snap to containers); selection endpoints snap unit-deep with raw blank-gap endpoints. Explicit
`reflow_buffer`/`unwrap_buffer`/`ventilate_buffer` (Format menu + palette) carry whole-document
intent through the same guards; the ctrl-t chooser is unchanged (keys mean unit scope).
Conventions worth knowing: every unit span extends to its line start (repar reflows at the
right content column); uncovered NON-blank lines (link-reference definitions get no block from
the parser — found by a gate probe as a fragment-mangling regression and fixed) widen selection
endpoints to whole lines, so repar never sees a mid-line fragment; the mid-tab nested-item
shape (`"- x\n\t- a"`) degrades to the OUTER item (pulldown can't split a tab byte —
ratified accept-and-pin). Suite 1,048; both final gates GO/READY; smoke 8/8; live two-scope
tmux sanity.

**Original facts:** all three share `region_for_transform` (`transform.rs:84-92`): WITH a selection →
snapped OUTWARD to whole TOP-LEVEL blocks intersecting it; WITHOUT a selection → the FULL
BUFFER (`0..buf_len`).

**Decided (2026-07-03):**
1. **Empty selection → the block under the caret** (never the whole buffer). Whole-document
   intent becomes explicit: add `reflow_buffer` / `unwrap_buffer` / `ventilate_buffer`
   variants (palette + Format menu). Rationale: the least input should not produce the
   largest effect; an accidental whole-document Ventilate is a massive surprise diff.
2. **Snapping targets the DEEPEST enclosing block(s), not top-level** — a selection inside one
   list item transforms just that item; spanning three items touches those three. Applies
   equally to the caret default (deepest block under the caret). The nested tree already
   carries child spans; transforms must respect item markers/continuation indent at item
   scope (they already handle lists at whole-list scope — adjacent machinery).
   May stage: defaults first, deepest-snap second, within one effort.

### C2b — Repar 1.0 integration (width + fixups)
<!-- item: C2b -->

User-directed insertion (reconstructed from the par-command session's lost BTW questions).
Transforms format to `view.wrap_column` (default 72; the hardcoded width died; clamped
20..=9999 — repar's REAL parse ceiling, probe-discovered after the spec's "huge" reading
was falsified); "Set Wrap Column…" minibuffer setter (Settings menu, live measure rebuild,
persisted via Save Settings with parse-boundary normalization); the pinned
`none,all,prose,markdown` fixups baseline — `prose` fixed REAL ventilate→reflow
corruption (periods eaten at default width). Six contract pins freeze repar-1.0 behavior,
incl. the deliberate CJK trailing-space artifact. TWO UPSTREAM repar report candidates
recorded in the spec's Deferred: (1) one trailing space per double-width char per line
(markdown hard-break injection for CJK/emoji); (2) common-prefix inference mangles
anaphoric ventilated prose under every stack.

### C3 — Cross-app clipboard over SSH/tmux
<!-- item: C3 -->

**Shipped:** the full works-everywhere provider/fallback chain (design D). One pure
`resolve_provider(ClipEnv, forced) -> ProviderPlan{layer1, osc52}` drives: register (source of
truth) → local owner (`CommandBackend` shelling to wl-copy/xclip/xsel/win32yank/clip.exe, arboard
native on mac/Windows + Linux fallback, Null) → OSC 52 with tmux/screen DCS wrapping for the
remote/multiplexer path (bare sequences were being swallowed by tmux `set-clipboard=external`). A
`clipboard.provider` option (`Auto`/`Native`/`Osc52`/`Off`) conforms to the command-surface contract
(4 palette-only set primitives + a Settings cycle, one shared setter, Save-Settings round-trip). The
plan is cached off the per-keystroke hot path (recomputed only on a provider change). Gated: Codex
spec 3 rounds + Codex plan 3 rounds; 9 TDD tasks (each two-verdict reviewed); Codex pre-merge GO +
Fable whole-branch GO (2,304-combo `resolve_provider` sweep + cached-plan-coherence + byte-exact
OSC 52 probes). 918+278+42 tests green, clippy `--workspace` clean, smoke 8/8 PASS. Behavior change
(latent-bug fix): a bare-headless terminal now reports the clipboard *available* and emits bare
OSC 52 (old code cried "unavailable" while silently emitting OSC 52). **Deferred follow-up Minors
(non-blocking):** (1) forced-`Native` headless silent degrade — a worker-side degrade notice would
honor no-silent-UI more fully (spec-documented tail; forced = user's explicit choice); (2) `Off`↔`Osc52`
flips send a harmless redundant `SelectProvider(Null)` rebuild. **PRIMARY selection and OSC 52 read
remain deferred** (nice-to-have). Detailed design/plan: `docs/superpowers/{specs,plans}/2026-07-07-*c3*`.

<details><summary>Original diagnosis + provider-model framing (pre-ship, 2026-06-28 → 2026-07-07)</summary>

**Reassessed 2026-07-07** · Small (minimal fix) / Small–Medium (robust provider model)

**UPDATE 2026-07-07 (re-grounded):** the paste-IN half already shipped since the original diagnosis.
Bracketed paste is enabled (`term.rs:42` `EnableBracketedPaste`) and `Event::Paste(text)` is handled in
four input contexts (`app.rs:390/463/601/711` — editor / prompt / palette / search), so pasting INTO
wordcartel over SSH already uses the robust path the diagnosis prescribed. **Remaining work is copy-OUT
only:** `$TMUX` detection + DCS passthrough wrapping of the OSC 52 emit in `clipboard.rs::osc52_set`
(still a *bare* sequence today — no `$TMUX`/wrap), a `.tmux.conf` doc note (`set-clipboard on`), and
manual verification across the terminal × tmux × SSH matrix. Net: essentially ONE small pure-function
task (the DCS wrap — unit-testable) + a doc note + spot-checks; the S5 PTY smoke check already covers
OSC 52 → tmux buffer. Design is basically settled — a short brainstorm, not a full one. Pasted text is
untrusted input, but the M2 `submit_transaction` boundary already covers it.

**Provider-model framing (2026-07-07 research).** "Robust for a word processor" ⇒ do NOT hard-code one
mechanism — adopt the **provider/fallback chain** the mature editors converged on. wordcartel is well
positioned: `clipboard.rs` ALREADY has the `ClipboardBackend` trait seam (`ArboardBackend`/`NullBackend`/
`FakeBackend`) + the in-process `Register` (source of truth) + bracketed paste. So this is an EXTENSION
of an existing abstraction, not a rewrite.

- **The matrix to plan for (axes, and the nasty combos):** (1) local vs remote/SSH (OS clipboard vs
  terminal-only); (2) multiplexer — tmux AND GNU screen, each with its OWN passthrough wrap (tmux
  `\ePtmux;…\e\\`, screen `\eP…\e\\`; nested tmux double-wraps); (3) Linux display server — X11 vs
  Wayland, plus the **persistence-on-exit gotcha** (the selection is served by the source process, so a
  short-lived TUI can LOSE the copied data on exit unless a helper / clip-manager persists it) and X11's
  **two selections** (PRIMARY vs CLIPBOARD); (4) **Windows + WSL** (`clip.exe` / `win32yank`, WSLg); (5)
  uneven terminal OSC 52 — most support WRITE (copy) but REFUSE READ (paste) for security, some do none,
  and there are **size caps** (tmux + terminals truncate large payloads); (6) headless / no-`$DISPLAY` →
  degrade to register-only.
- **Prior art (reference is Neovim):** Neovim ships no clipboard — it auto-detects and delegates to a
  provider (`wl-copy`/`xclip`/`xsel`/`pbcopy`/`win32yank`/`tmux`), and **0.10 added a built-in OSC 52
  provider** that detects tmux/screen and wraps — exactly the C3 shape. Helix = `clipboard-provider`
  (wayland / OSC 52 termcode / pipe); Kakoune + Emacs delegate to xsel/xclip/pbcopy/wl-copy; tmux's own
  `set-clipboard on` + `Ms` capability is the user-config route. Universal lesson: provider chain +
  detection + fallback, register as source of truth (done), bracketed paste for remote paste-in (done).
- **Crates:** `arboard` (current local base — X11/Wayland/mac/Windows, images; persistence caveat
  applies) · `copypasta` / **`copypasta-ext`** (Alacritty's; ext adds X11 **fork-to-persist** + an OSC 52
  provider — the most directly relevant prior art for the persistence fix) · `wl-clipboard-rs` (fine
  Wayland control) · `x11-clipboard` (PRIMARY+CLIPBOARD). OSC 52 itself is small enough to keep
  hand-rolled (we do); no crate helps paste-over-SSH — that's bracketed paste (done).
- **The wordcartel plan (extend `ClipboardBackend` into a chain):** copy-out = `Register` (always) →
  arboard (local) → OSC 52 with `$TMUX`/`$STY`-aware tmux+screen wrapping (remote; detect
  `$SSH_TTY`/`$SSH_CONNECTION`). **Wayland persistence decision:** OSC 52 SIDESTEPS it (the terminal owns
  + persists the clipboard after wcartel exits), so preferring OSC 52 in more cases — or pulling in
  copypasta-ext's fork-persist — are the two options to weigh in the brainstorm. WSL is covered by OSC 52
  (Windows Terminal supports it) or a `clip.exe`/`win32yank` backend. Degrade headless → register-only
  (status already says "clipboard unavailable"); keep the `OSC52_MAX_ENCODED` cap + warn on truncation.
  **PRIMARY selection scoped OUT of v1** (nice-to-have).
- **Sizing:** the MINIMAL remaining fix (tmux/screen copy-out wrap + doc) is still Small; the ROBUST
  provider-chain version is Small–Medium — an extension of the existing seam, gated as ever by the
  terminal × tmux × SSH (× Wayland/X11 × WSL) verification matrix, which is mostly manual spot-checks
  (the OSC 52 wrap + detection are unit-testable; S5 smoke covers OSC 52 → tmux).

*(Original diagnosis 2026-06-28 against `wordcartel/src/clipboard.rs`; predates the niggle triage.)*
Cross-application copy/paste (wordcartel → local app, or vice versa) does NOT work inside an
SSH/tmux session; within-session copy→paste works (the in-process `Register` is the source of
truth). **Copy-out:** we emit a *bare* OSC 52 set-sequence + the arboard worker — bare OSC 52
is swallowed by tmux unless `set-clipboard on`, and some setups need the DCS passthrough
wrapper (`\ePtmux;\e…\e\\`); we do NOT detect `$TMUX` and wrap. (The PTY smoke suite's S5 now
verifies OSC 52 lands in a tmux buffer in the harness config — partial live coverage of the
happy path.) **Paste-in [now SHIPPED — see UPDATE above]:** `arboard` `get()` targets the *remote*
(empty) display over SSH; OSC 52 read is refused by most terminals — the robust path is the terminal's
own bracketed paste arriving as input. **Direction (agreed 2026-06-28):** its own effort — `$TMUX`
detection + passthrough wrapping, a `.tmux.conf` doc note, bracketed-paste handling [done], tested
across terminal × tmux × SSH combos. Kept separate from multi-buffer work deliberately.

</details>

### C4 — Close-buffer Save/Discard/Cancel prompt
<!-- item: C4 -->

**Shipped:** closing a dirty buffer raises `[S]ave & close · [D]iscard · [C]ancel` (the
Effort 6 gap closed). Id-carrying prompt actions + `PostSaveAction::CloseBuffer{id}` ride
the quit save machinery with its staleness discipline; a busy guard isolates the prompt
from other flows' pending state; quit supersedes — and cancels — a pending close.
**Conventions (user-ratified):** Discard LEAVES the swap (one discard convention with
quit — reopening offers recovery); NO keybinding — ctrl-w is taken (`expand_selection`
CUA / `scroll_line_up` WS), the binding decision moves to A5.

*(Original entry below is historical.)*

*(Effort 6 spec-conformance gap, deferral agreed 2026-06-28.)* `workspace::close_buffer`
REFUSES to close a dirty buffer (status: `"unsaved changes — save or discard first"` — safe,
never loses work) instead of the interactive **Save / Discard / Cancel** prompt the Effort 6
spec called for (Task 7 deferred it "to Task 8"; Task 8 built only the quit state machine).
**Direction (agreed):** a small standalone effort — add a `Prompt::close_confirm` + resolve
arm that REUSES the quit machinery's per-buffer save-on-close path (the same
`dispatch_save_then` + `pending_after_save` flow used by `ContinueQuitDrain`); `close_buffer`
raises it when the active buffer is dirty.

---

## Theme D — config & persistence

### D1 — Save settings from the session
<!-- item: D1 -->

"Save Settings" (Settings menu) writes a machine-owned `settings-overrides.toml` (XDG config
dir; header-commented, atomic 0600, wholesale rewrite) layered ABOVE hand files and BELOW
`--config` — hand-written files are never touched. The diff against a pre-overrides BASELINE
follows the **contradiction-only-removal law** (both user-ratified): a key is written on
divergence, KEPT when a project baseline merely coincides with it (cross-project saves can't
erase each other), and removed only when the user actively changed the value back — with a
per-key `--config` mask-guard (a masking layer can never manufacture an un-save; the theme
key's mask is provenance-typed: `name` OR `file`). Theme identity is provenance-based
(`File` vs `Builtin(name)`; picker Enter commits only if a preview actually applied — scheme-
name collisions can't drop a pick). Persisted inventory: preset, theme name, five view
toggles, `menu.bar`, `mouse.capture` — never patches, never state. Refusals: `--no-config`
and missing config dir (status-line copy pinned). Settings-vs-state line: settings = user
intent → overrides; state = machine bookkeeping → session.toml, unchanged.

**Original facts:** NO config write-back exists (config is read-only at startup; `config.rs` has only
`load()` + layer discovery). The session store (`state.rs:80-101`) writes STATE (cursor,
marks, folds, scratch) to `$XDG_STATE_HOME/wordcartel/session.toml` — not settings.

**Fork (D1-b is the working instinct):**
- **D1-a:** edit the loaded TOML in place with `toml_edit` (comment-preserving, delta-only).
- **D1-b (favored):** never touch hand-written files — write a machine-owned
  `settings-overrides.toml` layered ON TOP of the existing chain, plus a "Save current
  settings" command that diffs runtime state vs loaded config and writes only the delta. Hand
  files stay sacred; the overrides file is disposable and inspectable.

**Draw the settings-vs-state line explicitly:** settings = user intent (theme, keymap preset,
menu.bar mode, view toggles, export engine/typography) → config/overrides; state = machine
bookkeeping (resume position, marks, folds) → the existing session store. Carrier for A5's
persistence.

---

## Theme E — product identity / chrome

### E1 — Chrome/density presets (zen|full)
<!-- item: E1 -->

**SHIPPED 2026-07-07**, merged `--no-ff` @ f7b7b10 (branch effort-chrome-density-overlays) as a
FOLDED effort delivering **E1 + E2 + overlay/mouse completeness + menu windowing** (spec
`2026-07-06-wordcartel-chrome-density-and-overlay-completeness-design.md`). E1 part: `chrome =
zen|full` now drives a **data-driven density preset** (`ChromeBundle`/`apply_bundle` — no `if zen`
branching) that sets the menu bar, transient status line + scrollbar (new `TransientMode {Off,Auto,
On}`), centered measure, and word count; individual config keys override a preset per-element and
persist; the two new `[view]` keys round-trip via the diff-law. **Default status_line = On** (user
decision — preserve the always-shown idle line; Zen opts into Auto). Both final gates GO (Codex
pre-merge + Fable whole-branch, each catching a real cross-task bug — hidden-command dropdown click,
paint/hit-test menu_area drift). See [[wordcartel-chrome-density-overlays]] /
[[wordcartel-chrome-density-defaults]]. The original design notes below are retained for history.

One coherent concept instead of N toggles: `chrome = "zen" | "full"` presets that SET the
individual toggles (each remains individually overridable). Chrome inventory (facts):

| Element | Today | Toggle | Config |
|---|---|---|---|
| Menu bar | only while menu open | `menu` (F10) | A1 adds `menu.bar` (default `auto`) |
| Status line | always on, NOT togglable | — | **decided:** transient in full-zen (below) |
| Scrollbar | auto-show 1200 ms on activity | none | — |
| Wrap guide | off/on | `toggle_wrap_guide` | `view.wrap_guide` |
| Centered measure | off/on | `toggle_measure` | `view.measure` |
| Word count | off/on | `toggle_word_count` | `view.word_count` |
| Heading glyphs | per-theme | none | `[theme] heading_level_glyph` (default → ON, B3) |
| Wrap column | 72; guide + measure + TRANSFORM width (repar10) | `set_wrap_column` | `view.wrap_column` (persisted, 20..=9999) |

**Decided (2026-07-03): the status line becomes transient chrome in full-zen** — the
established pattern (scrollbar on activity, menu bar on dwell): the row sits empty/hidden
while idle; **any status message — errors above all — reveals it** for a grace period or
until the next keystroke; prompts/modals always show. The no-silent-UI invariant is
preserved by construction (a hide-outright option is rejected on principle).

**Keep writing MODES out of chrome:** focus-dim + typewriter are writing modes, not
visibility chrome — they don't belong to the zen/full axis. Zen = `auto` menu bar + transient
status + minimal chrome; full = `pinned` bar + scrollbar + guide/measure/word-count +
right-edge bar content (designed here) — "a complete word processing system in one gesture."

### E2 — Visual polish pass
<!-- item: E2 -->

**SHIPPED 2026-07-07** with E1 (merge f7b7b10). Delivered: **state-in-label menu rows** (`Word
Count: On` / `Chrome: Zen` / `Keymap: CUA` — text, no glyphs; the ✓/radio idea was rejected as a
GUI import that ports awkwardly to a TUI; the keymap radio collapsed to a single `keymap_next`
cycle row) and the **two-archetype styling language** (attached filled-panel dropdown vs bordered
floating overlays, reusing the shipped six-face chrome family). Also folded in from the 2026-07-06
gaps: overlay/mouse completeness (click-to-commit + click-away on every list overlay, no mouse leak,
stale-guards on click-apply, diag windowing) and menu dropdown windowing. Residual: the A3
palette-completeness invariant test + item-by-item menu-curation pass is NOT part of this (see A3).

Original triage (retained): Full-width bar fill (A2), dropdown borders/styling, palette styling,
**checkable/stateful menu items** (✓/radio for toggles + the active keymap preset — the menu model
must support state display), a consistent styling language. Goal: the full-chrome mode looks
*designed*, not assembled.

### E3 — Chrome theming coherence
<!-- item: E3 -->

**Shipped as one effort with E4:** the six-face chrome family (`Chrome`, `ChromeReverse` cue-only,
`ChromeSelected`, `ChromeMuted`, + NEW `ChromeOverlay`/`ChromeAccent`); `Theme::derive_chrome`
(split-direction RGB pre-blend ladder — bars sink toward black, overlays raise; contrast-clamped;
sentinel-fill so explicit constructor faces survive); the `[theme] chrome = full|zen` axis +
`toggle_chrome` with honest no-effect arms + per-field persistence; the render split (ChromeStyles
precompute + render_overlays.rs painters); overlay interiors/query lines fully themed; fg-only
borders (the phosphor halo fix); the status row a full-width bar matching the menu bar, with an
accent face for prompt-active states. All three original reports pinned by regression tests.

**Follow-ups recorded at ship (from the final gates):** (1) **the unpainted canvas** — SHIPPED (canvas effort, merge 4090c47, 2026-07-06: opaque/transparent toggle) — — document
cells/blank rows carry the TERMINAL's bg, not `base_bg`; the whole ladder renders relative to the
terminal background, and flexoki-dark as launch default means a light terminal gets light-on-light
(pre-existing, human decision: paint base_canvas across the frame vs OSC default colors vs document
the assumption); (2) Ansi16 light-arm ChromeSelected is color-identical to the fill (selection
color-invisible at 16-color on light canvases — follow-up: e.g. DarkGray selected bg); (3)
rosepine-dawn base02 ships the base16-YAML `#f2e9de` vs canonical `#f2e9e1` (selection/wrap-guide
only).

*(Original design notes preserved below for provenance.)*

#### E3 original notes — `needs-design` (superseded by the shipped design doc)

*(Added 2026-07-04. Facts as of `bd3b72c`. SHOULD PRECEDE E1/E2's preset work and E4 —
presets and new themes should build on a coherent chrome model, not before it. **This effort
also CARRIES the render.rs split** (1,064 prod lines — one draw path + a painter per overlay
+ shared geometry): E3 touches every overlay painter anyway, so it carves render into
canvas/chrome/overlay-painter modules in the same pass rather than paying the churn twice —
from the 2026-07-04 module-size audit, see Theme H.)*

**The user's reports, all grounded as real:** modal text doesn't follow theme colors; modals
aren't consistent with the menu bar; the menu bar and status bar don't match each other.

**Facts — the per-surface style inventory (render.rs:712-717 + :629/:646):** there are exactly
FOUR chrome faces (`Chrome`, `ChromeReverse`, `ChromeSelected`, `ChromeMuted` —
wordcartel-core theme.rs:35-38; no overlay-specific faces). Their use is inconsistent:
- Overlay UNSELECTED rows (palette/outline/theme-picker/file-browser/diag):
  `RStyle::default()` — literally UNTHEMED (terminal default fg/bg after a `Clear` reset);
  in tokyo_night the row interior shows the terminal bg, not the theme's panel bg.
- Overlay query lines: `SE::Text` = `Face::default()` in every built-in — also unthemed.
- Overlay borders: `SE::Chrome` (themed) — so borders match the bar but interiors don't.
- Status bar + search/minibuffer/prompt: `SE::ChromeReverse` — a MODIFIER-ONLY reverse
  (ambient-dependent), while the menu bar uses explicit `SE::Chrome` colors. In tokyo_night
  the status row visibly does NOT match the menu bar's `PANEL_BG` — exactly the reported
  mismatch. There is no distinction between normal-status and active-prompt states.
- tokyo_night's liked "dark menus" = simply `chrome.bg = PANEL_BG (#16161e)` vs
  `base_bg (#1a1b26)` (theme.rs:322-325) — the subtle-dark chrome treatment to generalize.

**Direction (user-driven, 2026-07-04):**
1. **One chrome family, applied to EVERY chrome surface** — menu bar, dropdowns, all modal
   overlays (bg/rows/query), and the status bar draw from the same per-theme chrome palette
   (likely: overlay interiors get `Chrome`-family bg, rows get explicit styling instead of
   `RStyle::default()`, the status bar moves off modifier-only `ChromeReverse` onto explicit
   Chrome colors; whether four faces suffice or one or two are added is the design's call).
2. **A chrome DISPOSITION axis, proposed as config rather than name proliferation** —
   `[theme] chrome = "full" | "zen"`: `full` extends the theme's colors through all chrome
   (the new main tokyo-night); `zen` turns all chrome down to the subtle dark treatment
   (today's tokyo-night menus, generalized to every theme). Implementation is cheap by
   construction: a post-selection face patch in `resolve_theme` (the `override_face`
   mechanism already exists — theme_resolve.rs:84-138). AXIS-VS-VARIANT-NAMES is proposed,
   not yet user-confirmed — confirm at the E3 brainstorm.
3. **Phosphor restructure (user decisions 2026-07-04):** the `-flat` variants are REMOVED
   (their inversion — hue chrome over untinted text — defeats the point; two small sites:
   `builtin()` + `builtin_names()`, theme.rs:144-170; config naming them warns + falls back).
   The main phosphor themes already tint all text (`text: shade(hue,3)` — user-confirmed the
   full-illusion text side works); E3 completes the "one-color monitor" simulation by
   extending the hue through ALL chrome surfaces. The zen axis gives phosphor-text-over-
   subdued-chrome — what `-flat` should have been.
4. **tokyo-night full** — the main theme extends its palette through the menu bar, menus,
   modals, and status bar; today's subdued look becomes its `zen` disposition.

### E4 — Bundled themes
<!-- item: E4 -->

**Shipped:** ten bundled themes (catppuccin-mocha/latte, flexoki-dark/light, gruvbox-dark/light,
rosepine-moon/dawn, solarized-dark/light — all MIT, source URLs in theme.rs), `terminal-ansi`
(named-ANSI markdown colorization), `default` renamed `terminal-plain` (alias + warning at
resolve), phosphor `-flat` variants removed (chrome now derives), **flexoki-dark = the launch
default** (no-config arm; `Depth::None` still wins). 19 builtins total.

#### E4 original notes (provenance)

*(Added 2026-07-04, user request.)* Research additional out-of-the-box full-color themes.
Selection bar: **demonstrated markdown-colorizing strength** first; then portability into the
face model (base16-style mappings are cheap — `from_base16` exists), licensing, and terminal
ubiquity. Starting points supplied by the user:
- https://www.tabnine.com/blog/top-themes-for-sublime-text-editor/
- https://terminalroot.com/top-8-best-color-themes-for-your-vim-neovim/
Obvious candidates the research should weigh: Gruvbox, Catppuccin, Nord, Dracula, Solarized,
Everforest, Rosé Pine. Deliverable: a shortlist with per-theme markdown-rendering evidence +
face-model mapping notes, for the user to pick from. New themes land into E3's restructured
chrome model (hence the ordering).

---

### E5 — Chrome text intensity recede
<!-- item: E5 -->

**Observation (user):** the menu bar and status bar are visually distinct panels, but their **text is
the same color and intensity as the document body text**, so the *type* on the bars doesn't read as
chrome. Consider stepping the foreground intensity down on both so the bars are clearly not content.

**Grounded (may drift):** `Theme::derive_chrome` (`wordcartel-core/src/theme.rs:239`) builds the
chrome fg from `derive_fg(base_fg, bg)` (`:300`, `:323`) — it seeds from **`base_fg` (the document
body-text color)** and only nudges it enough to clear the `FG_FLOOR` = 4.5 legibility floor
(`:383`) against the *elevated panel background*. So the elevation currently lives entirely in the
BACKGROUND ladder (bar/dropdown/overlay step away from the canvas), while the chrome TEXT stays at
body-text hue/weight. On themes where the bg step is subtle, the bars read as same-weight text on a
faint panel. (terminal-plain/terminal-ansi use a reverse/explicit-named treatment instead — separate
path; no-color uses modifiers.)

**Direction (forks remain):** give chrome foreground a deliberate "recede" step below document
`Text` — e.g. a dim/lower-contrast target (still ≥ the 4.5 floor), or a muted-fg tone — so menu/status
type is legibly secondary. Note there is already a precedent: `ChromeMuted` (dropdown/secondary) is
dim; this would extend a similar intent to the primary bar/status fg. Constraints: stay ≥ the
legibility floor `derive_chrome` enforces; compose with density presets (E1, zen/full), the active-
prompt accent (`ChromeAccent`), the Ansi16 fixed policy, and no-color/mono (no hue to dim → needs a
modifier like DIM). Extends the **already-shipped** E3 chrome model (`derive_chrome`) — specifically
its fg-derivation step, which today seeds chrome text from body `base_fg`. Anchors:
`derive_chrome`/`derive_fg`/`FG_FLOOR` (`wordcartel-core/src/theme.rs`), the six chrome
`SemanticElement`s, E1.

### E6 — Splash / start screen
<!-- item: E6 -->

**Idea (user):** a splash / start screen showing the app name and information (version, quick help, maybe
recent files).

**Grounded (may drift) — tension with Theme E's ethos to reconcile:** Theme E is "minimalist by default,
complete on demand," so a splash must fit that — most likely shown ONLY on an empty/no-file launch,
dismissed on the first keystroke, never blocking instant typing (à la Vim's intro), OR gated behind a
setting. Content candidates: app name/tagline (see the naming lore), version (`pkgver`), a few key bindings,
open/recent affordances. Open Qs: empty/scratch launches only vs always; static vs interactive (recent-files
list); replace vs overlay the empty canvas. Must vanish the instant the user acts. Anchors: the
empty-canvas / no-buffer startup render path (`render.rs`), the app name/tagline (memory:
`wordcartel-tagline`), the version string.

### E7 — Grammar/spelling as a deliberate Analysis view (F1 RenderMode); draft stays quiet
<!-- item: E7 -->

**Product stance (user, 2026-07-11).** This is a writer's tool; grammar is an EDITING function, not a
drafting one (WordStar / G.R.R. Martin lineage — the machine gets out of the way while you generate). So
live, debounced grammar/spelling squiggles are the wrong default. Drafting stays quiet; grammar + spelling
become a **deliberate Analysis view** the writer summons.

**Mechanism — a 4th F1 RenderMode.** F1 = `cycle_render_mode` cycles `RenderMode { LivePreview,
SourceHighlighted, SourcePlain }` (`editor.rs:45`). Add an **Analysis** variant: the editable text with
grammar+spelling issues marked inline, navigable + quick-fixable via the EXISTING `diag_next`/`diag_prev`
+ quick-fix overlay. F1 in → issues appear; F1 out → gone. Analysis is a lens you look through, not chrome
over the draft. Exhaustive-`RenderMode` matching makes the variant compiler-safe to add. Command-surface:
a new render mode = a set-per-state primitive ("go to Analysis view") + the existing cycle (contract law 8).

**Latency is acceptable here — the key unlock.** "Instant typing, no silent UI waits" is a DRAFTING-path
invariant, not universal. Confining grammar to a deliberately-entered view relocates the checker's costs
(spawn, cold-init, round-trip, async) to a lag-tolerant place; results can even STREAM in naturally
("analyzing… N issues"). The view must render text immediately and decorate as results arrive — never block
on entering.

**Backend — external `harper-ls` LSP (this is the H2 → option D outcome).** Because latency now lands where
it's fine, consume Harper via its language server (`harper-ls`) as an external `optdepends` subprocess (like
pandoc/xclip) instead of embedding `harper-core`. Sheds the ~389-crate tensor stack from the binary (H2
spike 2026-07-11: 672→283 crates, 24→8.1 MB, ~4.5× build). Fits the existing async job substrate
(`diagnostics_run.rs` already spawns + version-discards) and Effort-P's job/plugin spine. If `harper-ls`
isn't installed, the Analysis stop degrades gracefully (skipped / "install harper-ls").

**Sequencing (de-risks it — two wins, shipped separately):**
1. **View first, embedded backend.** Move the Harper we ALREADY embed off the live debounce and into the
   Analysis view (runs only when in-view). Delivers the draft-quiet product change + the view UX at
   near-zero risk, no new subsystem — drafting goes quiet immediately.
2. **LSP backend swap (its own effort).** Replace embedded `harper-core` with a `harper-ls` LSP client
   (subprocess lifecycle, JSON-RPC, graceful degradation) — the dependency-weight shed (H2 → D), landed
   once the UX is validated.

**Out of scope (deliberately):** word statistics / readability / goals / streaks are AGGREGATE metrics (a
readout/panel, different data shape + cadence) — NOT this view. They belong to the **PA** analysis-plugin
cluster (post-Effort-P). Spelling rides WITH grammar in the view (one Harper pass), so drafting is fully
quiet by default; a passive live typo-mark could be a later opt-in without touching this design.

**Relates to:** H2 (backend → D), PA (stats as a plugin), R1 (removes grammar from the drafting hot path
entirely). Anchors: `RenderMode`/`cycle_render_mode` (`editor.rs:45`, `registry.rs:201`), `diagnostics_run.rs`
(async substrate), `diag_next`/`diag_prev` + quick-fix overlay, `docs/design/command-surface-contract.md`
(multi-state option pattern).

**Follow-ups (whole-branch review, 2026-07-11).** The final gates surfaced three non-blocking Minors: (a)
`set_render_mode` should close an open `DiagOverlay` on leaving Review (one-line `self.diag = None`;
currently effectively unreachable via real input since the open overlay consumes all keys, so cosmetic);
(b) lazy Harper `FstDictionary` warmup on first Review entry instead of at startup keyed on
`diag_cfg.enabled` (draft-quiet completeness; pairs with the E7-seq2 / H2 backend work); (c)
`recheck_diagnostics` is a silent no-op outside Review — give it a status hint ("no silent UI").

---

## Theme H — code health / engineering health

### H7 — Panic-safety & arithmetic-soundness audit
<!-- item: H7 -->

**SHIPPED 2026-07-10** (merge `a49743e`, branch effort-h7-panic-arithmetic-audit, 4-task subagent-driven
execution). Governed by the **blast-radius stance** (memory: [[wordcartel-h7-blast-radius-stance]]) — guard
strength matches what breaking an invariant costs the *user*, not a uniform fail-loud. Outcome vs. the
original ".unwrap() audit" framing: the panic surface was already ~90% disciplined (57/59 production
`.unwrap()`s guarded-by-construction, all 13 `.expect()`s informative, the 1 production `panic!` a
triple-gated `WCARTEL_SMOKE_PANIC` hook compiled out of release, **0 genuinely-fallible** unwraps), so the
real content was the **arithmetic-soundness sweep**. What landed:

- `shift_offset(pos, delta)` in `block_tree.rs` — clamps the 8 incremental-region offset sites
  (`debug_assert!` loud in dev/fuzz + safe release clamp-at-0) instead of wrapping a negative sum to a
  garbage `usize`. All 8 sites PROVEN non-negative under the enclosure invariant `region_old_end >= edit_hi`
  (lemma L1, traced through every mutation of the region bounds), so the release clamps are dead code unless
  the cross-crate `(ChangeSet, Edit)` contract breaks. Mutation-path release `assert!`s
  (`buffer.rs`/`change.rs`) left byte-untouched — a corrupt document position there stays loud, forever.
- **Core cast-soundness gate:** `wordcartel-core` denies `clippy::cast_possible_truncation`/`cast_sign_loss`/
  `cast_possible_wrap` (crate attribute in `lib.rs` beside `forbid(unsafe_code)`) with 7 minimal
  reason-carrying item-local `#[allow]`s — locks the class against regrowth. Shell deliberately not gated
  (~70 benign terminal-coordinate widenings).
- `build_multi_replace` degrades any malformed edit list (empty / reversed / overlapping / out-of-bounds)
  to an **identity no-op** before reaching `ChangeSet::from_ops`' release assert — hardens the boundary
  ahead of Effort P's untrusted plugin callers. Subsumes the 2 previously-flagged `.unwrap()`s.
- `place_cursor`/`screen_pos` clamp cursor-column narrowing (guard-before-narrow / saturating) instead of
  truncating a >65535-column position into view — cosmetic blast radius → silent clamp, no assert.

Non-goals honored: no `BytePos` newtype; no bulk `.unwrap()`→`.expect()` conversion of the guarded sites
(the full 59-unwrap / 13-expect / 1-panic classification appendix is recorded in the spec). Process: Fable
authored spec+plan (Codex-gated each round — spec READY r2, plan GO r2; see
[[wordcartel-fable-authors-codex-gates]]); 4/4 per-task reviews clean; both final gates GO (Fable
whole-branch review with a compiled 20-combination probe of the T3 no-op `Edit` through the T1-hardened
parse path; Codex pre-merge, zero findings). Core lib+F2 oracle + shell (969) tests green, workspace clippy
clean, smoke 8/8 PASS. Deferred informational Minor: the `block_tree` band-clamp comment's "cannot invert"
nuance (partial insurance under a broken I0 where the document shrinks below the region start — no worse
than pre-H7, degrades via `panicx::catch`) — a half-line comment amendment whenever the file is next
touched. See `docs/superpowers/{specs,plans}/2026-07-10-h7-panic-arithmetic-audit*.md`.

### H1 — God-object SEAM decomposition (app.rs/render.rs)
<!-- item: H1 -->

**SHIPPED 2026-07-09** (merge `304e263`, branch effort-h1-god-object-decomposition, 12-task subagent-driven
execution). The hub SEAM refactor landed behavior-identically: `run`'s 8-deadline loop → the `timers.rs`
**static fn-pointer table** (`SUBSYSTEMS` + `next_wake`/`on_tick`/`pre_recv`; gates + fire-order preserved;
idle-blocks/no-spin **proven by a compiled Fable probe** holding None at +24h); `reduce`'s ~900-line match →
a **10-stage `Handled`-protocol skeleton** + `fold_and_continue`, plus `Input(Key)` → `input::handle_key`;
the leaf extractions (`theme_cmds.rs`, `chrome.rs`, `chrome_geom.rs`, `render_status.rs`, + the micro-leaves
to their domain modules) and `list_window::apply_list_nav`. New guardrail pins assert the idle-blocks
invariant + the version-hook asymmetry. Both final gates GO (Fable whole-branch + Codex pre-merge); 1,267
tests green, clippy clean, smoke 8/8. Process: Fable authored the spec+plan (Codex-gated each round);
see [[wordcartel-fable-authors-codex-gates]].

**REMAINING — the one deferred piece, its own next effort: split the 522-line `render()` body** by paint
surface (row loop → `paint_rows`, status → `paint_status`, cursor → `place_cursor`; unify the twin
`segs`/`placed` span-builders). Deliberately scoped OUT of the shipped effort (which did verbatim render-*helper*
moves only): it is a different risk class — real restructuring that churns the pixel-exact golden-render tests —
so it earns a focused pass of its own. `render.rs` is not fully decomposed until this lands; low context
overlap with the app.rs work is exactly why it was split off (user decision 2026-07-09).

**Line anchors (2026-07-09 map; may drift — `render()` body is `render.rs:216–737`, guarded by the
`#[allow(clippy::too_many_lines)]` at :215).** The body is 12 sequential phases; the three split targets and the
dedup target:
- **`paint_rows`** ← the row loop `render.rs:358–606` (the mass). Per visual row it builds spans through two
  near-duplicate paths: the **segs** path (:395–450, no search/diag/sel/block) and the **placed** path (:451–588,
  per-glyph MarkedBlock→Selection→Search→Diag layering + run-accumulation).
- **`paint_status`** ← the status-line block `render.rs:635–699` (search bar / minibuffer / prompt / normal +
  right-flushed Ln/Col·words via `render_status::` helpers).
- **`place_cursor`** ← the hardware-cursor block `render.rs:704–734` (search field / minibuffer / `nav::screen_pos`).
- **Unify `segs`/`placed`:** the **prefix lead-in is near-verbatim duplicated** — segs at `render.rs:404–432`,
  placed at `:477–503` (same heading-numeral-box-vs-dim-glyph logic, copy-pasted), and both share the identical
  `row_dim`/`plain_source` compose ladder (:434–446 vs :515–527). That shared lead-in + style ladder is the
  natural extract. Everything after the loop is already delegated: `ChromeStyles::build` (:612, shared with
  `render_overlays.rs`), scrollbar (:617–630), and `render_overlays::paint` (:736).

*(Original triage below retained for history — the SEAM direction it sketched is what shipped.)*

`app.rs` is **5,519 lines** and `render.rs` **3,393** — past the point where one person holds them in
context. This is the clearest structural debt, and it bites hardest right before **Effort P**: `app.rs`
is where plugin/automation wiring lands, so a plugin surface bolted onto a 5k-line reducer is a
comprehension and review hazard.

**Real surface (2026-07-08 measurement — most of the line count is co-located tests):**
- `app.rs`: **~1,946 production** lines (~3,573 tests). The mass is two hubs — `reduce` (~900 lines) and
  `run` (the event loop, ~430) — plus ~25 small helpers.
- `render.rs`: **~1,028 production** (~2,365 tests), 26 paint fns. → **~3,000 production lines total**, not ~9k.

**Why it regrew (this drives the design).** The prior H1 pass (2026-07-04/05, commits `4e12212`…
`5c908f3`, all "verbatim move") extracted cohesive *leaves* — `jobs_apply.rs`, `session_restore.rs`,
`prompts.rs`, `search_ui.rs` — but deliberately left the two **hubs** (`reduce`, `run`) behind. Those
are exactly what every new interactive feature must touch, so app.rs grew **+814 lines in ~3 days**
(4,705 → 5,519) as D1/A5, E3/E4, the scrollbar/menu/status/mouse chrome, C3, R1, and the swap fix each
landed the same three shapes into the hubs: (1) a new `Msg` variant → a new `reduce` match arm; (2) a
new timed feature → a new `*_deadline` term in `run`'s loop (now **8** deadlines) + a `recompute_*`/
`*_tick` helper; (3) co-located tests. Not sloppiness — structural: the wiring belongs at the hub, and
the hub was left monolithic. **Effort P will do the same (plugin message-arms + hooks).**

**Direction — the durable fix is a SEAM, not more leaf extraction** (leaves alone regrew once already):
- **`run`'s deadline loop → a registry of timed subsystems** — each contributes its own deadline + tick,
  so a new timed feature registers a subsystem instead of editing the loop. Must preserve fire-order
  (dwell/grace first), the per-subsystem in-flight/pending gating, and the never-spin / idle-is-free
  invariant. Keep subsystems as free fns over `&mut Editor` (as today) to avoid borrow-checker friction.
- **`reduce`'s ~900-line match → per-domain handler modules** — keep the skeleton (prologue capture →
  modal/minibuffer/overlay interception chain → dispatch → epilogue: version-change hook + drain-fold);
  lift out the per-`Msg` handler bodies. The interception layering + shared epilogue are the careful
  part (ordering bugs here shift behavior).
- Finish the remaining **leaf extractions** (theme-cmds, chrome-`recompute_*`, session-persist, overlay
  dispatch) and split **render.rs** by paint surface — these are the easy, verbatim-move tasks.

**Difficulty: focused Medium.** Leaf extraction is trivial/low-risk. The two hubs are the real work —
`reduce`'s interception layering (harder) and `run`'s deadline-registry (invariant + borrow care).
Correctness risk is **low** (behavior-identical; caught by compiler + ~925 shell tests + the e2e
`reduce→advance→render` journeys + PTY smoke) — the cost is iteration-to-green, not debugging. The one
genuine risk is a **subtle emergent regression in the hub that no test covers** (exactly the swap-thrash
class), so it needs: a whole-branch review gate AND a new guardrail asserting the refactored `run` loop
still **blocks when idle** (the resource-behavior invariant).

**When (decision 2026-07-08): DEFERRED until Fable credits are back.** A hub refactor of the dispatch/
event loop is precisely the case Fable's executable whole-branch probes are worth spending on (the
subtle-emergent-behavior risk above); the user chose not to attempt it until then. Still gated **before
Effort P**. Not urgent for correctness.

### H4 — PKGBUILD pandoc + TeX optdepends
<!-- item: H4 -->

The Arch `PKGBUILD` (`packaging/arch/PKGBUILD`) lists optdepends for clipboard (wayland/libxcb/libx11/
wl-clipboard/xclip) but **not pandoc**, even though export shells out to it: `wordcartel/src/export.rs`
runs pandoc for html/docx/pdf export. It is genuinely *optional* — `probe_pandoc()` is cached and
returns false when pandoc is absent, and callers gate on it and show a status instead of failing — so
the right declaration is an **optdepend**, not a hard `depends`: `pandoc: markdown export (html/docx/pdf)`.
The **PDF** path additionally needs a TeX engine — the pandoc `--pdf-engine` defaults to xelatex
(`config.rs:139`) — so a second optdepend is likely warranted (e.g. `texlive-xetex: PDF export via
pandoc --pdf-engine=xelatex`). Direction: add both to the PKGBUILD optdepends when next touched; confirm
the exact Arch package names for the TeX engine. Anchors: `packaging/arch/PKGBUILD`,
`wordcartel/src/export.rs`, `wordcartel/src/config.rs:139`.

### H6 — Point-release version scheme + release process
<!-- item: H6 -->

**SHIPPED 2026-07-09** (branch release-v0.1.0-versioning, merge `50b449a`; design doc
`docs/superpowers/specs/2026-07-09-point-release-versioning-design.md`). Resolved all four forks below:
(a) **scheme** = SemVer pre-1.0 `0.MINOR.PATCH`, `1.0.0` reserved for the Effort-P capstone (MINOR = features,
PATCH = fixes-only); (b) **source of truth** = Cargo `[workspace.package] version` (both crates inherit via
`version.workspace = true`), a git tag `vX.Y.Z` mirrors it; (c) **PKGBUILD** `pkgver()` is now tag-anchored
(`git describe --tags | sed …` → `0.1.0` at the tag, `0.1.0.rN.gHASH` between); (d) **ritual** = a hand-curated
`CHANGELOG.md` (Keep a Changelog) + a 5-step release checklist in the design doc. Also closed the "app can't
report its version" gap: new `wcartel --version` / `-V` reads `env!("CARGO_PKG_VERSION")`. First release **v0.1.0**
cut against the current tree (annotated tag on `50b449a`) and the `release-dist` Arch package built
(`wordcartel-0.1.0-1-x86_64.pkg.tar.zst`). Gates green (build/clippy/`cargo test --workspace` all suites);
`--version` verified. Advances **H4** (packaging). *(Original triage below retained for history.)*

**Question (user):** decide on a point-release and versioning SYSTEM for the app.

**Grounded (may drift):** there is NO semantic version today — the Cargo crates are `version = "0.0.0"` and
the Arch `PKGBUILD` uses a VCS-style `pkgver()` = `0.0.0.r<commits>.g<hash>` (`packaging/arch/PKGBUILD:37`),
so every build is a git-describe snapshot with no human-meaningful release points. A point-release system
means choosing: (a) a scheme (SemVer `0.x`→`1.0` aligned to the Effort-P 1.0 capstone? CalVer?); (b) the
canonical home for the version (Cargo workspace `version`, a `VERSION` file, or git tags); (c) how the
PKGBUILD `pkgver()` derives from it (tag-based `git describe` instead of a raw commit count); (d) a
tag/changelog/release ritual. Ties to the 1.0 framing (Effort P = the 1.0 capstone per CLAUDE.md) and the
H4 packaging work. Anchors: `Cargo.toml` (`version`), `packaging/arch/PKGBUILD` (`pkgver()` :37), git tags
(none today).

### H8 — Remove dead fold/outline accessors
<!-- item: H8 -->

**SHIPPED 2026-07-09** (branch chore-h8-remove-dead-accessors). Fable scoped it (compile-verified a scratch
removal against the branch) and both accessors were deleted with their exclusive tests: `outline::section_range`
(+ tests `section_range_stops_at_same_or_higher_level`, `section_range_last_heading_runs_to_eof`) and
`fold::FoldState::hidden_byte_ranges` (+ test `hidden_byte_ranges_cover_body_not_heading`). Fable also caught two
live stale doc-comment references the grep pass missed (`outline.rs` `ordered` and `sections` doc comments named
the removed fns) — both fixed. Gates green: build + clippy `--workspace --all-targets` clean; `cargo test`
`wordcartel-core`/`wordcartel` all suites pass (core 279, shell 939, oracle 42, 0 failed). Low-risk, mechanical
as predicted; no shared test helpers over-deleted (`ordered`, `DOC`, `parse`/`doc` retained). *(Original triage
below retained for history.)*

**Grounded (rust-analyzer call-hierarchy + `findReferences` + raw grep, 2026-07-09).** Two `pub` fns are
referenced ONLY by their own unit tests — superseded-but-not-removed API. Both are the byte-space /
single-shot sibling of a batch API that the real hot path uses instead:

1. **`outline::section_range`** (`wordcartel-core/src/outline.rs:75`) — referenced only inside its own file:
   the def plus 4 unit-test call sites (lines 195, 198, 201, 209). Its doc comment describes the fold
   subsystem as the caller ("callers hide the body… and keep the heading visible"), but folding actually
   uses the `sections`/`body_range` batch API — `section_range` looks like a leftover from before that batch
   API landed. Tests to remove with it: `section_range_stops_at_same_or_higher_level`,
   `section_range_last_heading_runs_to_eof` (`outline.rs:187`/`:204`).

2. **`fold::FoldState::hidden_byte_ranges`** (`wordcartel/src/fold.rs:113`) — the BYTES-space hidden-range
   accessor; grep finds only the def and one test (`fold.rs:395`, `hidden_byte_ranges_cover_body_not_heading`).
   The per-frame path uses `FoldView::compute` (LINE space, merged + `epoch`-cached via
   `editor.active_fold_view()`) instead, so the byte-space variant is never invoked in production. Same
   superseded-sibling shape as (1).

**Direction when picked up:** for each, confirm no Effort-P/plugin surface is expected to want it, then delete
the fn + its test(s) — or, if it's meant to be kept as public API for plugins, add a production caller or a
`#[doc]`/rationale so it isn't mistaken for dead code. Low-risk, mechanical. Anchors: `outline.rs:75` (+`:187`/
`:204` tests); `fold.rs:113` (+`:395` test); the batch APIs that actually feed the hot path
(`outline::sections`/`body_range`, `fold::FoldView::compute`).

### H15 — app.rs/render.rs leaf extraction (first pass)
<!-- item: H15 -->

*(Renumbered from the old ux "Theme H · H1" to `H15`: the engineering-health `H*` namespace is the single home for code-health items; the god-object SEAM decomposition that superseded this leaf pass is eng-health `H1`.)*

*(Added 2026-07-04 from a module-size audit. "H" to avoid colliding with the hardening
campaign's F-numbering.)*

**Facts (as of `9d55a96`):** `wordcartel/src/app.rs` is 5,174 lines (~2,380 production,
188 fns) — six distinguishable modules wearing one name, with clean seams visible in its own
layout: (1) job/message application — the `apply_*` family, :118-:460; (2) session/resume
restoration — `apply_resume`/`load_*_from_entry`/`restore_*`/`open_into_current`, :407-:530;
(3) prompt submits & file dialogs — save-as, block-write, the 110-line `resolve_prompt`,
goto-line, :532-:767; (4) search-and-replace UI driving — nine `search_*` fns +
`diag_apply_selected`, :878-:1090; (5) `reduce` itself (~750 lines, :1104-:1836); (6) the run
loop machinery — `step`/`run`/`advance`/the deadline array/`recompute_*`/
`reconcile_mouse_capture`, :1850-:2380. Every 2026-07 effort (export, themes, menu modes) had
to edit it — the standing merge-conflict hotspot, and the hardest file for reviewers and
implementer subagents to hold.

**Direction:** a mechanical, behavior-preserving split along those seams — `jobs_apply.rs`,
`session_restore.rs`, `prompts.rs`, `search_ui.rs`, with `reduce` + the run-loop machinery
remaining as the residual `app.rs`. Pure `pub(crate)` moves, NO logic changes; tests move
with their subjects; suite green + clippy deny are the gates; `git blame -C -C` preserves
blame across moves (a split is a copy, not a rename — `git log --follow` does not apply).
**Timing:** before Effort P — the plugin event-hook dispatch seam lands
in exactly `reduce`/registry territory, and P's diff should land in a file whose main content
IS `reduce`, not line 1,104 of a six-topic file.

**Deliberate non-splits from the same audit:** `block_tree.rs` (1,318 prod, the second-
largest) stays WHOLE — it is the fuzz-hardened, oracle-anchored highest-bug-surface module;
the splice/widen/full-parse are one algorithm and the campaign's value is partly blame
stability there; restructure only if a B-strong-class parser replacement ever happens.
`nav.rs` (942) and `editor.rs` (767) are coherent — fine as-is.

---

### H16 — active_line end-of-buffer clamp
<!-- item: H16 -->

*(Discovered during B1+B2's e2e work, 2026-07-04; pre-existing, verified at source.)*
`derive.rs:217` computes the active line from `caret_byte.min(buf.len()-1)` — with a
trailing newline and the caret at `buf.len()`, the LAST CONTENT line stays active (renders
raw: no conceal, no glyph) even though the caret conceptually sits on the phantom line past
the newline. Latent UX quirk (the just-typed final list item never shows its bullet until
the caret moves up); orthogonal to wrap. Fix shape when picked up: treat caret==buf.len()
after a trailing newline as "past the last line" for active-line purposes.

### H9 — Lift logical-line helpers out of derive
<!-- item: H9 -->

**Grounded (rust-analyzer call-hierarchy + raw grep, 2026-07-09):** `derive::{total_logical_lines, line_start,
line_text}` (and the render-mode mapper `line_render_for`) are pure logical-line/line-space utilities with no
dependence on the derive PIPELINE — yet they live in `derive.rs` (the 979-line recompute module) and are the
most cross-imported thing in it. `line_start` alone has ~30 call sites across `nav.rs` (heavily), `render.rs`,
and `transform.rs`; `total_logical_lines` is used from `nav.rs`/`prompts.rs`/`commands.rs`; `line_text`/
`line_render_for` from `nav.rs`. So the whole nav/render line-space layer imports `derive` only to reach three
trivial helpers, coupling it to the parse/layout hub. Direction when picked up: lift the trio (plus
`line_render_for`) into a small `lines.rs` (or a `buffer`-adjacent home), leaving `derive` to own only the
`rebuild`/`rebuild_downstream` pipeline + `LayoutKey`. Mechanical (move + re-point imports; no behavior change),
low-risk, and a natural seam to take **alongside the remaining H1 `render()`/module-size work** rather than as a
standalone churn. Anchors: `wordcartel/src/derive.rs:91` (`total_logical_lines`), `:104` (`line_start`), `:116`
(`line_text`), `:25` (`line_render_for`); heaviest consumer `nav.rs`.

### H11 — Decompose commands::run god-function
<!-- item: H11 -->

**Grounded (read of `commands.rs:209–687`, 2026-07-09).** `commands::run` — the `Command`-enum dispatcher every
built-in and every registry handler ultimately routes through — is **478 production lines** carrying
`#[allow(clippy::too_many_lines)] // command dispatch — a flat table, one arm per Command variant`. But unlike the
genuine tables (`registry::builtins` = pure data rows; `reduce` = thin delegations; `timers::SUBSYSTEMS` = a
fn-pointer table), its arms are **inline edit bodies**, not delegations — so it violates the letter of the
*"a match arm is a thin delegation into a domain module — never an inline body"* anti-regrowth rule (CLAUDE.md →
Module structure) while claiming the flat-table exception. This is the same god-*function* class as the remaining
H1 `render()` split, and it's on the **Effort-P hot path** (plugin-invoked edits route through `run`), so it is
worth decomposing before P.

**Two low-risk moves, both preserving the flat-dispatch shape:**
1. **Factor the repeated edit epilogue.** Every edit arm ends with the verbatim
   `derive::rebuild(editor); nav::ensure_visible(editor); editor.active_mut().desired_col = None; CommandResult::Handled`
   (across `InsertChar`/`InsertNewline`/`Backspace`/`DeleteForward`/… ~8+ arms), and each first branches
   selection-vs-collapsed identically. One helper (e.g. `apply_edit_and_settle(editor, txn, edit, kind, sel, clock)`)
   collapses both — exactly the move `app::fold_and_continue` made for `reduce`'s "21 verbatim repetitions."
2. **Lift the edit-arm bodies into an `edit` module** (insert/delete/replace-selection primitives over the
   `ChangeSet`/`Edit`/`Transaction` builders already in this file — `replace_changeset`, `build_range_replace`,
   `build_multi_replace`), leaving `run` a thin exhaustive dispatch like `reduce`.

**Difficulty: focused Medium.** Behavior-identical; caught by the ~55 `commands::run` unit tests + the e2e
journeys. Lower risk than the H1 `render()` split (no pixel-exact golden churn — the edit paths are asserted by
buffer state, not rendered cells). Pairs naturally with the H1 module-size pass. Anchors: `wordcartel/src/commands.rs:209`
(`run` + the allow at `:210`), the changeset builders at `:101`/`:128`/`:156`, the repeated epilogue visible at
`:224–226`/`:238–240`/`:271–273`/`:287–289`.

### H14 — Split the render() body by paint surface
<!-- item: H14 -->

Split 522-line render() into paint_rows/paint_status/place_cursor; unify segs/placed lead-in. (H1 remainder.)

### H2 — Interrogate the `burn`/`harper` dependency weight
<!-- item: H2 -->

**Resolved (Effort A, 2026-07-11):** adopted option D — Harper consumed via the external `harper-ls` LSP
subprocess; the `burn`/`harper-core` tensor stack removed from our build (Cargo.lock 675→287, −388 crates).

`harper-core` grammar/spell checking drags in `burn` (a tensor/ML framework) + `cubecl` (its
GPU-compute layer, incl. CUDA) via `harper-brill` (Harper's POS tagger). It is used **in-process**
as a library (no LSP; `wordcartel-core/src/diagnostics.rs` calls `harper_core` directly on a worker
thread) — which is *why* the whole stack compiles into our binary.

**QUANTIFIED — spike 2026-07-11 (stub out `diagnostics::check`, drop the `harper-core` dep, measure):**

| | crates | clean release build | `wcartel` binary |
|---|---|---|---|
| with Harper | 672 | 50 s | 24 MB |
| without Harper | 283 | 11 s | 8.1 MB |
| **Harper's cost** | **+389 (58% of the lockfile)** | **~4.5× (+39 s)** | **~3× (+16 MB)** |

Plus a **runtime cold-init**: the first grammar check is **~2.34 s in a debug build / ~0.30 s
release** (warm ~18.6 ms debug / ~3.1 ms release, off-thread). This corrects the earlier "runtime
binary unaffected" note — the binary is 3× larger, and the debug cold-init is what a user hit as
typing "hitches"/multi-Enter (release is smooth; see R1). So the cost is build-time + supply-chain
+ binary-size + a one-time cold-init — matters more once **Effort P** opens a plugin attack surface.

**Cannot be trimmed at Harper's level (verified).** `harper-brill` is a **non-optional** dependency
of `harper-core` 2.x (no `optional = true`, unlike the adjacent `harper-thesaurus`); `default-
features = false` drops only the thesaurus, not `burn`. So keeping `harper-core` means keeping the
tensor stack. Also note (a) `diagnostics.grammar = false` only **filters grammar output** — Harper
still runs the full pass (for spelling), so it's a *distraction* toggle, not a cost toggle; only
`diagnostics.enabled = false` actually stops the work; (b) the config's `linters: Option<Vec<String>>`
field is **parsed but never read** — inert placeholder, no diagnostic source.

**Options (the decision):**
- **A — keep Harper, feature-gate the embed.** Gate the `harper-core` dep + `diagnostics` behind a
  cargo feature in `wordcartel-core` (default on) so a lean/`--no-default-features` build sheds the
  283→ stack. Lowest effort; two build flavors. Pairs with a runtime enable/disable toggle.
- **B — replace, spell-only** (e.g. `spellbook`, a pure-Rust Hunspell). Drops the whole stack;
  **loses grammar** (repetition/agreement/capitalization). Product decision: do we want grammar at all?
- **C — replace, rule-based grammar** (`nlprule`, pure-Rust LanguageTool rules, no tensors). Grammar
  without `burn`, but less maintained, sizeable rule bundles, lower coverage than Harper. Spike-worthy.
- **D — consume Harper via `harper-ls` (external LSP subprocess)**, the way VS Code / Neovim / Zed /
  Helix do. The tensor stack leaves *our* binary entirely; grammar becomes an `optdepends` external
  tool (like pandoc/xclip in the PKGBUILD), Harper updates decouple from our releases. Cost: an
  LSP-client + subprocess subsystem in the shell (fits the Effort-P job/plugin substrate). The
  architecturally "correct" way to shed a heavy *optional* capability.

**Key product fork:** how much do we value the grammar checking itself? If it stays → A (stopgap) or
D (durable). If spelling is what matters and grammar is marginal → B. **When:** opportunistic; pairs
with the pre-Effort-P dependency/audit pass (**H18**). Near-term mitigation shipping-independent of
this: a runtime `toggle_diagnostics`/`toggle_grammar` command (small, command-surface item).

### H18 — Supply-chain audit (cargo audit / cargo deny)
<!-- item: H18 -->

**Shipped (Effort A, 2026-07-11):** `deny.toml` cargo-deny config (advisories/licenses/bans/sources)
added; documented as a release-checklist step.

**Grounded (2026-07-10):** no `deny.toml`/audit config exists today, and the lockfile is large (672
crates — much of it the `burn`/`harper` tensor stack; see H2). Before Effort P opens an untrusted plugin
surface, run a supply-chain pass: `cargo audit` (RUSTSEC CVEs) and/or `cargo deny` (advisories + license
policy + duplicate/banned-crate checks), and decide whether to wire it as a CI gate. **Pairs with H2**
as the pre-P dependency pass, but on a distinct axis — H2 = build-time weight, H18 = vulnerabilities /
licenses. Forks: audit-only vs a full `deny` policy; gate vs advisory. Anchors: `Cargo.lock`, H2.

### H17 — Pre-P public-API doc-coverage sweep
<!-- item: H17 -->

**Shipped (2026-07-11, merge 11408b8):** documented all **237** undocumented public items in
`wordcartel-core` and enabled `#![warn(missing_docs)]` on the crate root (teeth via the warning-free-build
merge gate; `warn` not `deny`). Last of the pre-Effort-P sequence (A → B → H17). Calibrated pipeline:
Codex-gated spec + plan, subagent-driven per-file doc tasks (cheapest-model implementers) + per-task
doc-quality reviewers, single Codex pre-merge gate — no Fable. The review layer caught 3 real doc
*inaccuracies* the compiler can't see (`History::last_evicted` reset timing; `commit_coalescing`
`last_ms: now` vs `commit`'s `0`; `Diagnostic` sort/render misattribution), each fixed + re-reviewed.
Scope was core-only — the shell crate's ~660 internal-`pub` items were deliberately deferred (a
`pub → pub(crate)` visibility-tightening pass is the better home). Gates: missing_docs 0/0 (build +
`test --no-run`, covering the `cfg(test)` `test_support` fields), workspace clippy clean, full suite + 8
doctests green, smoke 9/9, Codex pre-merge GO.

**Grounded (2026-07-10):** the house style requires a doc-comment on every public item, but coverage
wasn't enforced — `wordcartel-core` exposed ~180+ undocumented `pub fn/struct/enum/trait/const/type`.
Effort P exposes this surface to plugins, so it should be documented and kept documented;
`#![warn(missing_docs)]` is a gate in the same spirit as the backlog drift gate and `module_budgets`.

### H12 — PTY smoke suite has no live-splash coverage (S9)
<!-- item: H12 -->

**Shipped (Effort B, 2026-07-11):** PTY smoke S9 live-splash check added (`start_wcartel --with-splash`,
`run.sh` s9 glob); smoke 9/9.

**Grounded (2026-07-10, splash effort merge 242c987).** The startup splash covers the first frame on every
launch and would fail all 8 PTY smoke first-frame checks, so `scripts/smoke/tmux-drive.sh`'s `start_wcartel`
now passes `--no-splash` on EVERY launch (alongside `--no-config`). Necessary — the smoke checks assert on
first-frame content and the splash is not what they test — but the side effect is that **no smoke check ever
exercises the real splash or its dismissal** (the Fable whole-branch review flagged this as advisory M-C). The
splash IS covered by in-process e2e journeys (`wordcartel/src/e2e.rs`: show-on-first-frame, key/mouse dismiss,
`--no-splash`, recovery-suppression at render level) and unit tests, so this is a *live-binary* coverage gap,
not a correctness gap. Direction when picked up: add an **S9** check that launches WITHOUT `--no-splash` and
asserts the real journey — wordmark/tagline on the first frame → a key dismisses it → the editor (or the opened
file) is revealed — plus optionally a swap-recovery-relaunch variant asserting the recovery prompt wins over
the splash on the live binary (the in-process e2e + the controller's manual PTY repro already proved this;
S9 would lock it into the mandatory-run suite). Low-risk, additive (one new check script + the launcher already
supports per-launch args). Anchors: `scripts/smoke/tmux-drive.sh` (`start_wcartel`, the `--no-splash` default),
`scripts/smoke/checks/`, `wordcartel/src/splash.rs`, the e2e journeys in `wordcartel/src/e2e.rs`.

### H5 — App-managed cleanup of swap files / state-dir debris?
<!-- item: H5 -->

**Shipped (Effort B, 2026-07-11):** `Clean recovery files…` command — command-only, fail-closed,
no-data-loss (assess-vouched deletions, TOCTOU-safe snapshot + confirm-time re-verify). No launch
auto-prune.

**Question (user):** should there be an in-app way to clean up swap files and other filesystem debris,
or is that something the user does outside the program?

**Grounded (may drift):** the app writes crash-recovery + session state under the XDG state dir
(`~/.local/state/wordcartel`, `swap::state_dir`): per-doc `*.swp` (hashed path), scratch
`scratch-{pid}.swp`, `session.toml`, and — as the swap durability work surfaced — occasional orphaned
atomic-write `*.tmp` files and stale swaps (e.g. the intentionally-left stale swap after a SaveAs
rekey, or scratch swaps from crashed sessions). `swap::find_orphan_scratch_swap` already scans for
crashed-scratch orphans on launch (for *recovery*), but nothing *prunes* accumulated debris. Forks to
weigh: (a) auto-prune on launch (delete swaps whose owning pid is dead AND whose doc is clean/unchanged);
(b) an explicit command (`Clean recovery files…`); (c) leave it to the user + document the dir. Ties to
the swap durability model (memory: `wordcartel-swap-idle-thrash`). Anchors: `wordcartel/src/swap.rs`
(`state_dir`, `swap_path`, `find_orphan_scratch_swap`), `recovery.rs`.

## Theme R — responsiveness

### R1 — Typing latency + double-Return / line-jump
<!-- item: R1 -->

**Shipped:** the three per-keystroke/per-caret-move O(document) outline/fold walks are guarded on
`FoldState::is_empty()` (behavior-identical — skips only provably-empty work): `FoldView::compute` trivial
early-return, `rebuild_downstream`'s reconcile gate (`!folds.is_empty()`), and `fold::normalize_caret`
(Component 4, the caret-navigation walk = symptom 2, folded in post-Fable). Plus `first_frame_settle` (a
`LayoutKey`-gated rebuild after `ensure_visible`) fixing the T5 first-frame startup staleness. Three
`#[cfg(test)]` walk counters lock the invariant in. Root cause: `blocks_generation` conflated
text-change with structure-change, defeating the outline/fold memoization. Measurement-driven (5-agent
code-map → burst bench quantifying the O(block-count) slope → fuzz sweep confirming no pulldown panic);
both final gates GO (Fable proved no navigation regression byte-for-byte vs the pre-guard baseline); 925
shell + 278 core + 42 oracle green, clippy clean, smoke 8/8. The investigation record below is retained.
**Deferred (recorded):** input coalescing (bench-supported burst contributor; touches the input loop +
no-silent-UI); the reconcile-debounce retiming (measurement showed it's off-thread in production — moot).

*(Facts as of `86db660`. This is the durable record of the diagnostic work — theories tried,
refuted, and confirmed — for three symptoms the user reported while writing in wcartel. It is
NOT yet a spec; a tight-scope effort is being brainstormed off it. Gating note: per the
2026-07-06 work-style change, Codex is the sole spec/plan gate; Fable reviews the whole branch
only.)*

**The three reported symptoms (user, 2026-07-06):**
1. **Double-Return** — "after some lines I need to hit Return twice to create a line break."
2. **Line-jump** — "starting a line immediately under the line above jumps the second line
   below the preceding line."
3. **Typing jerk** — rendering "halts / catches / jerks" as it types into the buffer.
Plus a high-value diagnostic clue: 1-3 appear right after a buffer is **first opened** and
after a **theme change**, but largely disappear when the user toggles **centered view**
(measure mode) in and out.

**Empirical anchor — the 50-Return test (real `wcartel` via tmux, both paced ~80 ms and fast
burst):** typed N Returns into a fresh empty buffer, saved, counted newlines. Result is
perfectly linear `N -> N+1` (0->1, 1->2, 3->4, 50->51; the +1 is the trailing newline of an
empty doc), **identical paced vs burst.** Conclusion: NO keystrokes are dropped, the buffer is
always correct, and there is **no data-loss risk** — the double-Return is a *rendering/timing*
effect (the newline IS inserted; the frame just doesn't show it, so the user presses again and
actually inserts a second newline — documents quietly accumulate extra blank lines).

**Theory ladder — what we tried, in order, with verdicts:**

- **T0 — "no hot-path latency by construction" (initial read).** INCOMPLETE, later corrected.
  Verified the reduce-loop input guards and the compose/render *styling* path stay O(1)/O(visible)
  after E3+E4. Did NOT audit the two places a per-keystroke O(document) cost hides: the
  incremental parse-on-edit *downstream reconcile* and the render diff. That blind spot is exactly
  where the real bug lives (T4).
- **T1 — stale width-keyed layout cache (from the measure-toggle clue).** REFUTED. `LayoutKey`
  (`wordcartel/src/derive.rs:9-20`) is nine fields incl. `blocks_generation` (content) and
  `heading_level_glyph` (theme) — not width-only. `toggle_measure` performs no reparse (parse is
  version-gated at `derive.rs:116`; the toggle only flips `view_opts.measure` at
  `registry.rs:387`). An in->out measure toggle returns every key field to its prior value, so it
  is net-zero on the layout cache and cannot "repair" it. A probe of 11 common edits found
  `incremental == full` (the known F2-oracle divergences are tail nested/loose-list shapes, not
  ordinary typing).
- **T2 — markdown soft-break / editing-model (raw-source vs rendered-view).** REFUTED by live
  repro: opening a file `AAA\nBBB` (single newline) renders AAA and BBB on **separate rows** — a
  single newline IS a visible line break at rest. wcartel does not collapse soft breaks the way
  raw CommonMark would, so the double-Return is not a semantics effect.
- **T3 — active-line reveal/conceal reflow (caret line shows raw markdown, others conceal).**
  NOT REPRODUCED for the tested case: a line with a long link renders raw whether the caret is on
  it or moved away, and the line below does not move. Remains a *possible* contributor for other
  markdown (headings/glyphs) but not the demonstrated cause.
- **T4 — per-keystroke O(document) fold/outline walks.** PROVEN — the root of symptom 3, and the
  likely upstream cause of 1-2 as lag. See mechanism below.
- **T5 — startup first-frame staleness.** CONFIRMED as the one genuinely separate correctness
  bug, but narrow (startup only — see below).

**Live visual repro findings (read-only, real binary):** (a) single newline renders as a line
break at rest; (b) typing "AAA" / Enter / "BBB" with settle-pauses renders each step correctly —
the double-Return is NOT reproducible once frames settle, i.e. it is transient/lag; (c) no
link-conceal reflow. **Net: symptoms 1-2 are transient lag artifacts, not a rendering-correctness
bug** — everything renders correctly the moment the app catches up, and the measure-toggle relief
is a full-repaint flushing transient on-screen state (NOT a cache repair).

**PROVEN mechanism (symptom 3 = T4).** `derive::rebuild_downstream`
(`wordcartel/src/derive.rs:183-274`) runs on EVERY keystroke and does two O(document) walks even
when zero folds are open:
- **Fold-anchor reconcile** (`derive.rs:193-196`): `outline::heading_starts`
  (`wordcartel-core/src/outline.rs:92`) builds the full `Vec<Heading>` via
  `ordered`->`headings`->`heading_title`, **allocating a `String` title per heading**
  (`outline.rs:24-62`), then keeps only the byte offsets and throws the titles away. Gated on
  `last_reconciled_generation != gen`, but `blocks_generation` bumps on every edit
  (`Editor::set_blocks`, `editor.rs:91-94`, unconditional), so the gate misses every keystroke.
- **Fold-view compute** (`derive.rs:201`): `Editor::active_fold_view` (`editor.rs:521-535`) is
  memoized on `(blocks_generation, folds.epoch())` — but generation bumps every edit, so
  `FoldView::compute` (`fold.rs:133`) walks `outline::sections` (another full-tree +
  String-per-heading pass) every keystroke.
Cost grows with document size -> "worse after some lines." The run loop has **no input
coalescing** (`app.rs:1592`/`:1624` — one `reduce` + one `draw` per message), so a fast typist
outruns the draws and the queue drains in bursts -> the catch/jerk feel.

**CONFIRMED narrow bug (T5 = startup staleness).** The sequence `rebuild -> ensure_visible ->
draw` at startup (`app.rs:1536-1547`) has NO rebuild between `ensure_visible` and the first draw.
`ensure_visible` (`nav.rs:401`, returns `()` today) mutates `view.scroll`/`view.scroll_row` but
does not refresh `line_layouts`, and render has no on-demand layout fallback — so if the caret is
off-screen on open, the first frame is built for the wrong visible range. Grounding showed this is
the ONLY genuine gap: everywhere reached through the reduce loop, `advance()` runs a pre-draw
`derive::rebuild` (`app.rs:1242`/`:1623`) that repairs scroll every keystroke — so theme-change
and active typing are already repaired, and their perceived jank is T4 (slowdown), not T5.

**Fix directions (grounded):**
- **Fix #1 (T4, the anchor):** guard both walks on "no folds active" (`FoldState::is_empty`,
  `fold.rs:19`) — the overwhelmingly common case does zero walks; and add a non-allocating
  byte-only `heading_starts` for when folds DO exist (skip the per-heading `String`). Likely
  resolves all three symptoms (jerk directly; 1-2 as lag downstream).
- **Fix #2 (T5):** have `ensure_visible` report whether it moved scroll; re-run `derive::rebuild`
  when it did, at the startup site (and defensively any draw path outside the reduce loop).
- **Latency probe (greenfield — no criterion/benches today):** a before/after wall-clock measure
  of the per-keystroke cycle at several document sizes, plus a deterministic regression guard
  extending the existing `LAYOUT_RUNS`-style instrumentation (`derive.rs:23-25`) to assert zero
  O(document) walks per keystroke when no folds are open.

**Proposed scope (tight, pending user confirmation):** Fix #1 + Fix #2 + the probe.
**Deferred** (recorded, not in this effort): (a) **input coalescing** in the run loop — smooths
bursts but touches the input loop + the no-silent-UI invariant, revisit only if typing still feels
bursty after Fix #1; (b) the **`area_height` inconsistency** (`derive.rs:212` uses full terminal
height; `ensure_visible` at `nav.rs:436` subtracts `1 + menu_bar_rows`) and `LayoutKey` omitting
`menu_bar_rows` — real but *over*-caches (harmless direction), not the bug.

**Recommended sequencing:** before Effort P — the plugin system only adds hot-path pressure, and a
latency probe is worth having in place before then.

**ADDENDUM 2026-07-07 (independent re-confirmation + two new mechanisms).** A fresh multi-agent
code-map of the per-keystroke path (5 module mappers + synthesis) independently re-derived T4 and it
was verified against the CURRENT source (line numbers have drifted since `86db660`): the two
whole-document walks now live at `derive.rs:208-211` (`heading_starts`) and `derive.rs:216`
(`active_fold_view` → `outline::sections`), both still gated on `blocks_generation` which
`Editor::set_blocks` bumps every edit (`editor.rs:91-93`) → the memoization is defeated on every
keystroke. Confirmed as a real O(document) per-keystroke tax, worst in large/heading-dense docs — the
"all three symptoms" baseline. Two mechanisms the earlier record did NOT isolate:
- **Reconcile debounce mistiming (NEW — but MEASUREMENT DOWNGRADED it).** The reconcile debounce is
  **150 ms** (`reconcile.rs:10`), shorter than a ~180 ms/key cadence, so it fires between keystrokes.
  The code-read feared a main-thread O(document) hitch — but the bench REFUTES that for production: with
  the real threaded `Executor`, `full_parse_rope` runs off-thread and the merge Tick stays **flat
  ~78–146 µs** vs N (the tree-eq compare short-circuits / is cheap). The O(document) ~3.7 ms@1M cost
  appears ONLY on the `InlineExecutor` (test-seam) path. Net: **not a production hot-path problem**;
  deprioritized. (Retiming the debounce above cadence is still a cheap tidy-up, not a fix.)
- **Block-tree widen/gap-materialization (NEW — the paragraph-end spike).** An Enter/blank-line near an
  absorptive container (list/quote/indented-code) far upstream makes the incremental update materialize
  the inter-block gap line-by-line (`block_tree.rs:719-723`) or `WidenToEnd` to EOF (`:875`) — O(document)
  on that single structural keystroke. Pins symptom 2 (paragraph endings) to a concrete mechanism rather
  than "lag downstream of T4."

**QUANTIFIED (burst-based bench, 2026-07-07, branch `effort-r1-typing-latency`, `6669e73`; release, e2e
seam, N×structure×edit-class, p99 log-log slope vs N).** The O(document) bug is CONFIRMED — and the model
sharpened: the walks are linear in **block/heading COUNT, not raw bytes**, so the clean linear slope shows
on **heading-dense (`heading_starts` 0.99 / `foldview` 1.03), nested-list (1.33 / 1.20), code-heavy (1.31 /
1.31)** — and reads FLAT on flat-prose (few big blocks) and giant-table (1 block) at the µs floor. The walk
fired on **200/200 Input frames** (memoization defeat proven directly). **Positive control HELD:**
`layout_fill` (0.05–0.14; ~300 µs, the largest phase, correctly O(visible)) and `render` (0.02–0.10) are
flat — the harness is valid. `parse` is linear on nested-list (0.87) / giant-table (0.93) — the
paragraph-end widen/gap finding, confirmed. **Bounding caveat:** at ≤1 MB NO cell breaches the 8 ms (120 Hz)
budget (worst total p99 ~6.3 ms @1M giant-table), so this is a **scaling risk + burst-backlog contributor**
(per-keystroke cost stacking faster than 16 ms frames drain, given no input coalescing), not a single-key
stall at present doc sizes. Raw CSV + slope table in `.superpowers/sdd/r1-bench{.csv,-slopes.md}` (gitignored).

**Cross-reference (2026-07-07 fuzz sweep):** `block_tree.rs`'s incremental machinery is a shared hotspot —
it underlies both R1's paragraph-end widen cost AND the still-open incremental≡full soundness divergences
(a ~43 M-exec sweep re-found the latter with fresh minimized repros in `fuzz/artifacts/block_tree/`; that
same sweep found **no** pulldown-cmark panic, so the M4-rest `catch_unwind` isolation is belt-and-suspenders).

## Theme M — hardening campaign

### M8 — M5 follow-up: undo louder-hint for buffer-level merges
<!-- item: M8 -->

**Shipped (Effort B, 2026-07-11):** per-buffer `undo_evicted_pending` surfaces buffer-level (non-active)
merge evictions after reduce + at `switch_to_index`.

Finish the louder undo-eviction hint for buffer-level merges (the last M5 follow-up).
