# UX backlog — niggling issues, grounded facts, design directions

**Origin:** 2026-07-03 triage session. Fourteen user-reported niggles, organized into five
themes, each fact-checked against the real source (anchors below are as of `63f98de` and may
drift). Each item graduates to the standard gated pipeline (brainstorm → spec → Codex/Fable
review → plan → subagent build) when picked up — this document is the durable triage, not a
spec.

**Status legend:** `settled-design` (direction agreed, ready to spec) · `needs-design`
(direction sketched, forks remain) · `needs-decision` (a human call blocks progress) ·
`available-today` (config-only, no code) · `fact-checked` (behavior pinned, no direction yet).

---

## Governing principle — the three-surface contract (adopted as working rationale)

wordcartel has three command surfaces. The contract that keeps them coherent:

1. **The registry is the single source of truth** (already true — everything routes through it).
2. **The palette is exhaustive** — every registered command appears unless explicitly flagged
   internal. *Already implemented:* `palette.rs:66-86` iterates the whole registry (~126
   commands); a pinned test asserts "empty query → all commands" (`palette.rs:138`).
3. **The menu is curated** — the ~58 commands tagged `menu: Some(category)` in `CommandMeta`
   (`registry.rs:45-48`; five categories File/Edit/Format/View/Export, tree built dynamically
   by `menu::grouped_commands`). Menu ⊆ palette, always.
4. **Both surfaces show live keybinding hints**, resolved against the *active* keymap (matters
   once presets switch at runtime — A5).

Deviations from this contract are treated as bugs. Corollary: every mouse affordance keeps a
keyboard path (the palette guarantee makes this cheap to honor).

---

## Theme A — command-surface architecture

### A1. Menu bar states + mouse reveal — `settled-design` · Medium

**Facts:** F10 is the only activation (`keymap.rs:243`); the menu opens straight to
pulled-down (first dropdown visible — `menu::empty()` + hydrate, `app.rs:824-826`); there is
no bar-visible-inactive state (bar renders only while `editor.menu.is_some()`,
`render.rs:906`); no visibility config key. **Mouse plumbing already complete** once a bar
exists: click bar label switches category, click dropdown row dispatches, click-away closes
(`mouse.rs:115-142`, hit-testing shared with the renderer).

**Settled design:**

```toml
[menu]
bar = "hidden" | "auto" | "pinned"   # proposed default: auto
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

**Dwell safeguards:** transit ≠ dwell; never arms during a drag (button held = selecting);
wheel events don't count; a click on row 0 while hidden is a TEXT click (reveal comes only
from dwell or F10); if mouse capture is off or the terminal lacks motion reporting, `auto`
degrades silently to `hidden` + F10.

**Implementation notes:** the scrollbar already does transient chrome (auto-show on activity,
self-hide 1200 ms — `recompute_scrollbar_visible`); the reveal timer is the same pattern with
a dwell trigger, slotting into the run loop's existing `recv_timeout` deadline array. Motion
events already flow through the mouse path (currently ignored).

### A2. Full-width menu bar + right-edge content — fill `trivial`, right side `needs-decision`

**Facts:** only the label rects get the Chrome style (`render.rs:108-118`, `:915-920`); gaps
and the right side are unstyled — no full-width fill.

**Direction:** fill row 0 with the bar background (trivial). What anchors the right side is a
deliberate chrome decision, not a "because it's there" fill — candidates: buffer name + dirty
marker, word count, active keymap preset. Default: background only; content belongs to the
chrome story (E1). Must decompose gracefully on narrow terminals (labels truncate before
content).

### A3. Palette completeness follow-ups — `fact-checked` · Small

The user's impression ("only a small subset") was inverted — the palette is already
exhaustive; the MENU is the subset. Follow-ups: (a) verify the palette shows binding hints
(menu is built with keymap access; palette unverified); (b) investigate why the palette read
as a subset (filter UX? discoverability?); (c) review the ~58-command menu curation
deliberately. Add a permanent **palette-completeness invariant test** ("every non-hidden
registry command appears") — the contract as a regression net (the `palette.rs:138` test is
close; formalize).

### A4. Menu accelerators (Alt+F/Alt+E…) — `needs-decision` · deferred

Optional classic accelerators that open a specific category directly. Conflicts to resolve:
Alt is used by fold (Alt+Z), and hints must be preset-aware. Not blocking anything; revisit
after A1 lands.

### A5. Switch keybind system from the menu — `needs-design` · Medium (ships with D1)

**Facts:** `build_keymap` runs exactly once at startup (`app.rs:2029`); no runtime rebuild
path; no switch command; presets = cua, wordstar. **Direction:** a `keymap_preset` command
(menu: View or a Settings home) → rebuild the trie between reduces in `run()` (flag/Msg-driven;
the trie is borrowed by `reduce`); menu hints stay fresh automatically (menu rebuilds on every
open); palette hints must re-resolve. Persistence rides on D1 — these two ship together.
Checkable/radio menu items (E2) show the active preset.

---

## Theme B — rendering fidelity

### B1. Word-boundary wrap — `needs-design` · Larger (highest-value rendering fix)

**Facts:** the soft-wrap is greedy PER-GRAPHEME (`layout.rs:261-292`): when
`col + vg.width > vw` the overflowing grapheme moves to the next row — no word-boundary
lookback/lookahead of any kind. Words break mid-word at the viewport edge.

**Direction:** break at whitespace with the standard overflow fallback (a single word longer
than the line still breaks). Touches the per-frame hot path (`layout()`) and ripples into
`ColMap`/caret/click mapping (the shelved-F8 territory — `docs` + memory record why
bound-to-visible-rows was rejected; word-wrap does NOT change that constraint, it changes row
break positions only). Should travel with hanging indent (B2's companion). CJK/no-space text
falls back to grapheme wrap. Pin with e2e Harness journeys (wrap + caret round-trip).

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

### B3. Heading glyphs in colored themes — `available-today` · Trivial

**Facts:** `Theme.heading_level_glyph: bool` (`theme.rs:119`); shade ramp `█ ▓ ▒ ░ ▏ ·`
H1→H6 (`render.rs:16-18`, gate at `:412-421`). ON for `no_color` + the phosphor-*flat*
variants; OFF for default/tokyo-night/base16. A config key already exists:

```toml
[theme]
heading_level_glyph = true
```

works with any colored theme TODAY (glyph renders in the heading's color). Residual effort:
decide whether colored themes default to `true` (one line per theme; taste).

---

## Theme C — document workflow

### C1. LaTeX export + xelatex PDF — `needs-design` · Small

**Facts:** export is already PANDOC-based and async (`export.rs`; html captures stdout,
docx/pdf via temp+rename, `Msg::ExportDone`, pandoc probed via OnceLock). **`export_pdf`
passes no `--pdf-engine`** → pandoc defaults to pdflatex — wrong for a xelatex user (fontspec/
Unicode documents will mangle or fail).

**Direction:** (a) `export_tex` via the same `run_export` machinery (`pandoc -s -t latex`);
(b) config: `export.pdf_engine = "xelatex"` (+ possibly `export.pdf_variables` for
letterpaper/mainfont) — fixes PDF and gives the .tex export its personality; (c) OPEN IDEA —
**export-time typography** (smart quotes/dashes at export only): rejected in-editor as
violating source-as-is, but export-time transformation is fully compatible with that
philosophy. Decide deliberately.

### C2. Transform scope (Reflow/Unwrap/Ventilate) — `needs-decision` · Small

**Facts:** all three share `region_for_transform` (`transform.rs:84-92`): WITH a selection →
snapped OUTWARD to whole TOP-LEVEL blocks intersecting it; WITHOUT a selection → the FULL
BUFFER (`0..buf_len`).

**So: yes, they work on selections (block-snapped).** Two open calls: (1) should
empty-selection default to the CURRENT BLOCK instead of whole-buffer (recommendation: yes —
keep explicit `…_buffer` variants for the whole-document intent)? (2) is whole-top-level-block
snapping acceptable for nested content (a selection inside one list item transforms the entire
list)?

---

## Theme D — configuration & persistence

### D1. Save settings from the session — `needs-design` · Medium

**Facts:** NO config write-back exists (config is read-only at startup; `config.rs` has only
`load()` + layer discovery). The session store (`state.rs:80-101`) writes STATE (cursor,
marks, folds, scratch) to `$XDG_STATE_HOME/wordcartel/session.toml` — not settings.

**Fork (D2 is the working instinct):**
- **D1-a:** edit the loaded TOML in place with `toml_edit` (comment-preserving, delta-only).
- **D1-b (favored):** never touch hand-written files — write a machine-owned
  `settings-overrides.toml` layered ON TOP of the existing chain, plus a "Save current
  settings" command that diffs runtime state vs loaded config and writes only the delta. Hand
  files stay sacred; the overrides file is disposable and inspectable.

**Draw the settings-vs-state line explicitly:** settings = user intent (theme, keymap preset,
menu.bar mode, view toggles, export engine) → config/overrides; state = machine bookkeeping
(resume position, marks, folds) → the existing session store. Carrier for A5's persistence.

---

## Theme E — product identity: minimalist by default, complete on demand

### E1. Chrome/density presets — `needs-design` · Larger (the umbrella)

One coherent concept instead of N toggles: `chrome = "zen" | "full"` presets that SET the
individual toggles (each remains individually overridable). Chrome inventory (facts):

| Element | Today | Toggle | Config |
|---|---|---|---|
| Menu bar | only while menu open | `menu` (F10) | — (A1 adds `menu.bar`) |
| Status line | always on, NOT togglable | — | — (full-zen may want it hideable) |
| Scrollbar | auto-show 1200 ms on activity | none | — |
| Wrap guide | off/on | `toggle_wrap_guide` | `view.wrap_guide` |
| Centered measure | off/on | `toggle_measure` | `view.measure` |
| Word count | off/on | `toggle_word_count` | `view.word_count` |
| Heading glyphs | per-theme | none | `[theme] heading_level_glyph` |

**Keep writing MODES out of chrome:** focus-dim + typewriter are writing modes, not
visibility chrome — they don't belong to the zen/full axis. Zen = `auto` menu bar + minimal
chrome (today's feel); full = `pinned` bar + scrollbar + guide/measure/word-count + right-edge
bar content — "a complete word processing system in one gesture."

### E2. Visual polish pass — `needs-design` · rides E1

Full-width bar fill (A2), dropdown borders/styling, palette styling, **checkable/stateful menu
items** (✓/radio for toggles + the active keymap preset — the menu model must support state
display), a consistent styling language. Goal: the full-chrome mode looks *designed*, not
assembled.

---

## Cross-cutting notes

- **Testing synergy:** every item lands with e2e `Harness` journeys (menu state machine,
  wrap/caret, palette-completeness invariant) and the PTY smoke layer covers mouse/real-
  terminal behavior (dwell-reveal is smoke-testable via tmux mouse events). The new
  infrastructure makes each of these cheap to pin.
- **Keyboard-reachability guarantee:** every mouse affordance has a keyboard path — enforced
  by the palette contract.
- **Preset-aware hints:** binding hints in menu/palette must re-resolve on keymap switch (A5).

## Open questions ledger

1. Menu curation: is the current ~58-command `menu: Some(_)` set the right curation? (A3)
2. `auto` as the default menu-bar mode — proposed, awaiting explicit confirmation. (A1)
3. Right-edge menu-bar content: what, if anything, by default? (A2/E1)
4. Empty-selection transform default: current block vs whole buffer? (C2)
5. Whole-top-level-block snap on nested selections: acceptable? (C2)
6. Export-time typography (smart quotes/dashes at export only): adopt? (C1)
7. Accelerators (Alt+letter): want them at all? (A4)
8. Status line hideable in full-zen? (E1)
9. Colored themes defaulting `heading_level_glyph = true`: which themes? (B3)

## Sizing summary

- **Config-only today:** B3 heading glyphs.
- **Trivial:** A2 bar fill · C1 `--pdf-engine=xelatex`.
- **Small:** C1 `export_tex` · A3 palette follow-ups + invariant test · C2 transform default.
- **Medium:** A1 menu bar modes + dwell (mouse comes free) · A5 keymap switch + D1 write-back
  (one effort) · B2 sub-list indent (+ hanging indent).
- **Larger:** B1 word-boundary wrap · E1/E2 chrome presets + polish pass (after A1/A2).
