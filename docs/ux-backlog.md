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

---

## Governing principle — the three-surface contract (adopted)

wordcartel has three command surfaces. The contract that keeps them coherent:

1. **The registry is the single source of truth** (already true — everything routes through it).
2. **The palette is exhaustive** — every registered command appears unless explicitly flagged
   internal. *Already implemented:* `palette.rs:66-86` iterates the whole registry (~126
   commands); a pinned test asserts "empty query → all commands" (`palette.rs:138`).
3. **The menu is curated** — the ~58 commands tagged `menu: Some(category)` in `CommandMeta`
   (`registry.rs:45-48`; five categories File/Edit/Format/View/Export, tree built dynamically
   by `menu::grouped_commands`). Menu ⊆ palette, always. **Curation principle (adopted
   2026-07-03):** the menu carries every command a word-processor user would go looking for by
   category — file ops, clipboard/undo, transforms, view toggles, export — plus anything whose
   discoverability matters; palette-only is for motions/navigation, internal plumbing, and
   keystroke-native commands; every toggle shown in the menu displays its state (E2 checkable
   items). The item-by-item pass applying this rule happens in the A3 effort.
4. **Both surfaces show live keybinding hints**, resolved against the *active* keymap (matters
   once presets switch at runtime — A5).

Deviations from this contract are treated as bugs. Corollary: every mouse affordance keeps a
keyboard path (the palette guarantee makes this cheap to honor).

---

## Theme A — command-surface architecture

### A1. Menu bar states + mouse reveal — `SHIPPED` 2026-07-04 (7273327) — hidden|auto|pinned, dwell+grace, click-to-open, menu_bar_pin; ALSO fixed the pre-existing nav menu-row geometry bug

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

### A2. Full-width menu bar + right-edge content — `SHIPPED` 2026-07-03 (097dcae)

**Facts:** only the label rects get the Chrome style (`render.rs:108-118`, `:915-920`); gaps
and the right side are unstyled — no full-width fill.

**Decided (2026-07-03):** fill row 0 with the bar background at all times the bar is shown —
**background only; no right-edge content by default.** Right-edge content (leading candidate:
buffer name + dirty marker) is designed once, deliberately, inside E1's full-chrome work — not
defaulted piecemeal now. Labels truncate before any future content on narrow terminals.

### A3. Palette completeness follow-ups + the menu item pass — `fact-checked` · Small

The user's impression ("only a small subset") was inverted — the palette is already
exhaustive in DATA; the MENU is the subset. Follow-ups: (a) verify the palette shows binding
hints (menu is built with keymap access; palette unverified); (b) **ANSWERED 2026-07-04:**
the subset impression was real in REACH — the palette cannot scroll past its initially
visible rows without typing (see A6, which fixes it); (c) **apply the adopted curation
principle** (see the contract section) item-by-item to the ~58-command menu set, bringing
only the judgment calls back for approval. Add a permanent **palette-completeness invariant
test** ("every non-hidden registry command appears") — the contract as a regression net (the
`palette.rs:138` test is close; formalize).

### A4. Menu accelerators (Alt+F/Alt+E…) — `dropped` (2026-07-03)

**Decided: no global Alt accelerators.** With the settled A1 model, any category is two
keystrokes away (F10 + arrows) or one dwell+click; the menu is a *discovery* surface — speed
users graduate to bindings and the palette. Global Alt+letter bindings cost real conflict
surface (Alt+Z is fold; `Edit`/`Export` collide on E; every preset inherits the reservations)
for a layer nobody has asked to use. **Revisit trigger:** actual user demand. The low-conflict
middle path — within-menu mnemonics while a menu is open — is recorded as an optional A1
garnish (see A1), not a commitment.

### A5. Switch keybind system from the menu — `needs-design` · Medium (ships with D1)

**Facts:** `build_keymap` runs exactly once at startup (`app.rs:2029`); no runtime rebuild
path; no switch command; presets = cua, wordstar. **Direction:** a `keymap_preset` command
(menu: View or a Settings home) → rebuild the trie between reduces in `run()` (flag/Msg-driven;
the trie is borrowed by `reduce`); menu hints stay fresh automatically (menu rebuilds on every
open); palette hints must re-resolve. Persistence rides on D1 — these two ship together.
Checkable/radio menu items (E2) show the active preset.
### A6. Palette reachability: full-list scrolling + wheel + click dead zones — `needs-design` · Small-Medium

*(Added 2026-07-04; answers A3(b). Facts as of `bd3b72c`.)*

**Facts:** the `Palette` struct has NO scroll field (palette.rs:19-28); render slices only the
FIRST `list_h` rows into the ratatui `List` (`rows.iter().take(list_h)`, render.rs:760-777)
with `list_h = min(row_count, 15, h-4)` (`palette_overlay_rect`, render.rs:150-159). Arrow
keys move `selected` over the FULL row set (app.rs:1240-1249, max = rows.len()-1 = ~125) but
ratatui can only scroll items it received — so past row 15 the highlight simply VANISHES,
and **Enter still dispatches the invisible selection** (a silent-wrong-action hazard, worse
than mere unreachability). PgUp/PgDn: no key arms at all. Mouse wheel: the palette overlay
block returns early for ALL mouse events (mouse.rs:122-145) — ScrollUp/Down never reach the
scroll arms. Click: CORRECT for visible list rows (`palette_row_at` matches the render
layout exactly, render.rs:163-174; test-pinned) — but clicks on the query row and border
cells are swallowed silently (inside the overlay → neither dispatch nor close), the likely
source of the "can't click" report.

**Direction:** add a scroll offset (`scroll_top`) to `Palette`; render slices
`[scroll_top .. scroll_top+list_h]`; selection movement scrolls the window to follow
(standard follow-the-cursor windowing); PgUp/PgDn arms; wheel events route to palette
scrolling while it's open (the overlay block handles them instead of swallowing); decide the
dead-zone behavior (query-row/border clicks: no-op is acceptable, but must not eat the
event silently if a better affordance is cheap). Kills the invisible-dispatch hazard by
construction (selection always visible). Same windowing likely wanted for outline/theme
picker/file browser (they share the pattern — verify in-effort and fix uniformly if cheap).


---

## Theme B — rendering fidelity

### B1. Word-boundary wrap — `needs-design` · Larger (highest-value rendering fix)

**Facts:** the soft-wrap is greedy PER-GRAPHEME (`layout.rs:261-292`): when
`col + vg.width > vw` the overflowing grapheme moves to the next row — no word-boundary
lookback/lookahead of any kind. Words break mid-word at the viewport edge.

**Direction:** break at whitespace with the standard overflow fallback (a single word longer
than the line still breaks). Touches the per-frame hot path (`layout()`) and ripples into
`ColMap`/caret/click mapping (the shelved-F8 territory — word-wrap does NOT change the
bound-to-visible-rows rejection, it changes row break positions only). Should travel with
hanging indent (B2's companion). CJK/no-space text falls back to grapheme wrap. Pin with e2e
Harness journeys (wrap + caret round-trip).

### B2. Sub-list bullet indent (+ hanging indent) — `needs-design` · Medium-small

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

### B3. Heading glyphs in colored themes — `SHIPPED` 2026-07-03 (097dcae): default ON in every theme

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

---

## Theme C — document workflow

### C1. LaTeX export + xelatex PDF + export typography — `SHIPPED` 2026-07-03 (097dcae) — and it was a BUG FIX (docx/pdf silently exported HTML fragments; confirmed + fixed)

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

### C2. Transform scope (Reflow/Unwrap/Ventilate) — `settled-design` · Medium

**Facts:** all three share `region_for_transform` (`transform.rs:84-92`): WITH a selection →
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

### C3. Cross-app clipboard over SSH/tmux — `needs-design` · Medium (pre-triage deferred, pulled in 2026-07-04)

*(Diagnosed 2026-06-28 against `wordcartel/src/clipboard.rs`; predates the niggle triage.)*
Cross-application copy/paste (wordcartel → local app, or vice versa) does NOT work inside an
SSH/tmux session; within-session copy→paste works (the in-process `Register` is the source of
truth). **Copy-out:** we emit a *bare* OSC 52 set-sequence + the arboard worker — bare OSC 52
is swallowed by tmux unless `set-clipboard on`, and some setups need the DCS passthrough
wrapper (`\ePtmux;\e…\e\\`); we do NOT detect `$TMUX` and wrap. (The PTY smoke suite's S5 now
verifies OSC 52 lands in a tmux buffer in the harness config — partial live coverage of the
happy path.) **Paste-in:** `arboard` `get()` targets the *remote* (empty) display over SSH;
OSC 52 read is refused by most terminals — the robust path is the terminal's own bracketed
paste arriving as input. **Direction (agreed 2026-06-28):** its own effort — `$TMUX` detection
+ passthrough wrapping, a `.tmux.conf` doc note, bracketed-paste handling, tested across
terminal × tmux × SSH combos. Kept separate from multi-buffer work deliberately.

### C4. Close-buffer Save/Discard/Cancel prompt — `needs-design` · Small-Medium (pre-triage deferred, pulled in 2026-07-04)

*(Effort 6 spec-conformance gap, deferral agreed 2026-06-28.)* `workspace::close_buffer`
REFUSES to close a dirty buffer (status: `"unsaved changes — save or discard first"` — safe,
never loses work) instead of the interactive **Save / Discard / Cancel** prompt the Effort 6
spec called for (Task 7 deferred it "to Task 8"; Task 8 built only the quit state machine).
**Direction (agreed):** a small standalone effort — add a `Prompt::close_confirm` + resolve
arm that REUSES the quit machinery's per-buffer save-on-close path (the same
`dispatch_save_then` + `pending_after_save` flow used by `ContinueQuitDrain`); `close_buffer`
raises it when the active buffer is dirty.

---

## Theme D — configuration & persistence

### D1. Save settings from the session — `needs-design` · Medium

**Facts:** NO config write-back exists (config is read-only at startup; `config.rs` has only
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

## Theme E — product identity: minimalist by default, complete on demand

### E1. Chrome/density presets — `needs-design` · Larger (the umbrella)

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

**Decided (2026-07-03): the status line becomes transient chrome in full-zen** — the
established pattern (scrollbar on activity, menu bar on dwell): the row sits empty/hidden
while idle; **any status message — errors above all — reveals it** for a grace period or
until the next keystroke; prompts/modals always show. The no-silent-UI invariant is
preserved by construction (a hide-outright option is rejected on principle).

**Keep writing MODES out of chrome:** focus-dim + typewriter are writing modes, not
visibility chrome — they don't belong to the zen/full axis. Zen = `auto` menu bar + transient
status + minimal chrome; full = `pinned` bar + scrollbar + guide/measure/word-count +
right-edge bar content (designed here) — "a complete word processing system in one gesture."

### E2. Visual polish pass — `needs-design` · rides E1

Full-width bar fill (A2), dropdown borders/styling, palette styling, **checkable/stateful menu
items** (✓/radio for toggles + the active keymap preset — the menu model must support state
display), a consistent styling language. Goal: the full-chrome mode looks *designed*, not
assembled.

### E3. Chrome theming coherence — one chrome family + a full|zen axis + phosphor restructure — `needs-design` · Medium-Large

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

### E4. Bundled themes research — `research note` · Small (research), lands AFTER E3

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

## Theme H — code health

### H1. app.rs decomposition — `settled-direction` · Medium (mechanical; do BEFORE Effort P)

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
with their subjects; suite green + clippy deny are the gates; `git log --follow` preserves
blame across moves. **Timing:** before Effort P — the plugin event-hook dispatch seam lands
in exactly `reduce`/registry territory, and P's diff should land in a file whose main content
IS `reduce`, not line 1,104 of a six-topic file.

**Deliberate non-splits from the same audit:** `block_tree.rs` (1,318 prod, the second-
largest) stays WHOLE — it is the fuzz-hardened, oracle-anchored highest-bug-surface module;
the splice/widen/full-parse are one algorithm and the campaign's value is partly blame
stability there; restructure only if a B-strong-class parser replacement ever happens.
`nav.rs` (942) and `editor.rs` (767) are coherent — fine as-is.

---

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

Hard edges: E3 → E1/E2/E4; D1 carries A5; E2 rides E1; H1 before Effort P. Soft edges: H1
relocates what C4/D1/A5/E1 touch (split early = every later effort lands in focused files);
B2's hanging indent is a wrap-policy feature (travels inside B1); A3's palette parts share
A6's territory; E2's checkable items serve A5/E1; C2 and C3 are islands.

1. **A6** palette reachability — folds in A3(a) hints-verification + the palette-completeness
   invariant test (same territory). Kills the invisible-dispatch hazard first.
2. **H1** app.rs decomposition — immediately after the small win; everything behind it lands
   in focused files.
3. **B1 + B2** word-boundary wrap + list indent, ONE effort (bullet indent + hanging indent
   inside the wrap work). The user's highest-value rendering fix; the E-arc then gets judged
   on correctly wrapped text.
4. **C4** close-buffer prompt — lands in the post-H1 prompts.rs, reuses the quit machinery.
5. **C2** transform scope — settled, independent.
6. **D1 + A5** settings write-back + keymap switch — the persistence rail BEFORE the settings
   that ride it (E3's chrome axis, E1's presets).
7. **E3** chrome theming coherence (+ the render.rs split; **E4's research kicks off in
   parallel** — pure reading, no code dependency).
8. **E1 + E2** density presets + polish — the convergence point (+ E4's theme landings +
   A3's menu-curation pass, done while the menu is being polished anyway).
9. **C3** SSH/tmux clipboard — genuinely independent; last by cost shape (a terminal × tmux ×
   SSH test matrix), not by value; pull it forward whenever the pain bites.

Then **Effort P**, landing on a decomposed app.rs, a coherent chrome model, and a settings
rail. FLAGGED JUDGMENT: B1 sits before the D/E arc on value; pure dependency logic permits
swapping blocks 3 and 6-8 if the visual-consistency wins should bank first — B1 is
dependency-free in both directions.

## Sizing summary

*(SHIPPED so far: the quick-wins bundle A2+B3+C1 @ 097dcae; A1 menu-bar modes @ 7273327 —
both carrying bug fixes.)*

- **Small:** A3 palette follow-ups (hints verification + invariant test + the menu item
  pass) · E4 themes research (the research itself).
- **Small-Medium:** A6 palette reachability (scrolling window + wheel + dead zones — also
  kills the invisible-dispatch hazard) · C4 close-buffer Save/Discard/Cancel prompt (reuses
  the quit machinery).
- **Medium:** A5 keymap switch + D1 write-back (one effort) · B2 sub-list indent (+ hanging
  indent) · C2 transform scope (block-under-caret defaults + deepest-block snapping) ·
  C3 clipboard over SSH/tmux (the terminal × tmux × SSH test matrix is the real cost) ·
  H1 app.rs decomposition (mechanical; before Effort P).
- **Medium-Large:** E3 chrome theming coherence (one chrome family + the full|zen axis +
  the phosphor restructure — precedes E1/E2 and E4's landings).
- **Larger:** B1 word-boundary wrap · E1/E2 chrome presets + polish pass (after A1/A2 and
  E3; includes the transient status line).
