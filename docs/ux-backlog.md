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

### A3. Option reachability + preset-aware hints (three-surface integrity) — `SHIPPED` 2026-07-07 (merge d7a5494)

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
whole-branch PASS (14 execution probes). Residual polish (non-blocking, deferred): `toggle_status_line`
`_ => "Auto"` catch-all → make exhaustive; `user_bound` fully-qualified `HashSet` path; a
palette count-assert overlap; asymmetric setter-test asserts; latent `chars().count()` vs display-width
in `right_justify_leaves`/`menu_dropdown_rect` (CJK-width — matches existing convention). The
original settled-design notes below are retained for history.



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

### A3b. Item-by-item menu-curation pass — `settled-principle` · Small (split from A3, 2026-07-07)

Apply the adopted curation principle (see the three-surface contract section) item-by-item across the
~126 registry commands / ~58 menu set: decide per command whether it belongs in the *menu* (by
category — the commands a word-processor user goes looking for) vs *palette-only* (motions,
navigation, internal plumbing, keystroke-native), bringing only the genuine judgment calls back for
approval. Lower-risk polish; rides whenever. Independent of A3 (A3 fixes the contract-*violation*;
A3b is the contract-*application* sweep). The state-in-label display (E2) is already done.

### A4. Menu accelerators (Alt+F/Alt+E…) — `dropped` (2026-07-03)

**Decided: no global Alt accelerators.** With the settled A1 model, any category is two
keystrokes away (F10 + arrows) or one dwell+click; the menu is a *discovery* surface — speed
users graduate to bindings and the palette. Global Alt+letter bindings cost real conflict
surface (Alt+Z is fold; `Edit`/`Export` collide on E; every preset inherits the reservations)
for a layer nobody has asked to use. **Revisit trigger:** actual user demand. The low-conflict
middle path — within-menu mnemonics while a menu is open — is recorded as an optional A1
garnish (see A1), not a commitment.

### A5. Switch keybind system from the menu — **SHIPPED 2026-07-05** (with D1, merged @ 4670eaf)

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
### A6. Palette reachability: full-list scrolling + wheel + click dead zones — `SHIPPED` 2026-07-04 (A6 T1+T2)

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


---

## Theme B — rendering fidelity

### B1. Word-boundary wrap — `SHIPPED` 2026-07-04 (with B2, one effort)

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

### B2. Sub-list bullet indent (+ hanging indent) — `SHIPPED` 2026-07-04 (with B1)

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

### B4. `SourceHighlighted` (SRC-HI) mode is a no-op — renders identically to `SourcePlain` — `SHIPPED` 2026-07-07 (merge 1bbd82b): uniform per-construct coloring in the current theme's faces

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

### C2b. Repar 1.0 integration (width + fixups) — **SHIPPED 2026-07-05** (merged @ c9b64d8)

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

### C2. Transform scope (Reflow/Unwrap/Ventilate) — **SHIPPED 2026-07-05** (merged @ 642290b)

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

### C4. Close-buffer Save/Discard/Cancel prompt — `SHIPPED` 2026-07-04

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

## Theme D — configuration & persistence

### D1. Save settings from the session — **SHIPPED 2026-07-05** (with A5, merged @ 4670eaf)

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

## Theme E — product identity: minimalist by default, complete on demand

### E1. Chrome/density presets — `SHIPPED` 2026-07-07 (merge f7b7b10): zen|full data-driven density presets

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

### E2. Visual polish pass — `SHIPPED` 2026-07-07 (merge f7b7b10, with E1)

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

### E3. Chrome theming coherence — `SHIPPED` 2026-07-06 (merge eb9cfd1, with E4)

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

### E4. Bundled themes research — `SHIPPED` 2026-07-06 (merge eb9cfd1, with E3)

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

## Theme H — code health

### H1. app.rs decomposition — `SHIPPED` 2026-07-04 — app.rs 4,346 lines (from 5,740); new modules jobs_apply.rs 496, session_restore.rs 309, prompts.rs 414, search_ui.rs 211

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

## Theme R — editing responsiveness (the project's #1 invariant: instant typing)

### R1. Typing latency + double-Return / line-jump — full investigation record — `in-brainstorm` (2026-07-06) · Medium

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

### S1. Rearrangeable outline / heading-subtree corkboard — `needs-design` · Medium

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

### S2. Directory-as-binder (project/manuscript over many files) — `needs-design` · Larger

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

### S3. Snapshots — named, durable revision checkpoints ("fearless editing") — `needs-design` · Small–Medium

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

### H2. active_line clamps to the last content line at end-of-buffer — `noted` · Small

*(Discovered during B1+B2's e2e work, 2026-07-04; pre-existing, verified at source.)*
`derive.rs:217` computes the active line from `caret_byte.min(buf.len()-1)` — with a
trailing newline and the caret at `buf.len()`, the LAST CONTENT line stays active (renders
raw: no conceal, no glyph) even though the caret conceptually sits on the phantom line past
the newline. Latent UX quirk (the just-typed final list item never shows its bullet until
the caret moves up); orthogonal to wrap. Fix shape when picked up: treat caret==buf.len()
after a trailing newline as "past the last line" for active-line purposes.

## Working order (recorded 2026-07-04 — dependency-derived, user-approved)

Hard edges: E3 → E1/E2/E4; D1 carries A5; E2 rides E1; H1 before Effort P. Soft edges: H1
relocates what C4/D1/A5/E1 touch (split early = every later effort lands in focused files);
B2's hanging indent is a wrap-policy feature (travels inside B1); A3's palette parts share
A6's territory; E2's checkable items serve A5/E1; C2 and C3 are islands.

*(Progress: 1 A6 ✓ · 2 H1 ✓ · 3 B1+B2 ✓ · 4 C4 ✓ · 5 C2 ✓ · 6 D1+A5 ✓ · 7 E3+E4 ✓ · **8 E1+E2 ✓**
(2026-07-07 @ f7b7b10 — folded effort: E1 + E2 + overlay/mouse completeness + menu windowing) ·
**B4 SRC-HI ✓** (2026-07-07 @ 1bbd82b) — **next: C3, R1 (in-brainstorm), or Theme S/P; then Effort
P**. A3's palette-completeness invariant + item-by-item menu-curation pass remains open.)*

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
9. **C3** SSH/tmux clipboard — genuinely independent; last by cost shape (a terminal × tmux ×
   SSH test matrix), not by value; pull it forward whenever the pain bites.

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

**SHIPPED** (see each item's header for the commit): A1, A2, A5, A6, B1, B2, B3, B4, C1, C2, C4,
D1, E1, E2, E3, E4, H1 — plus the repar re-plumb check. (A4 dropped.) The remaining open work,
by size:

- **Small:** A3 palette-completeness invariant test + the item-by-item menu-curation pass (the
  state-in-label half shipped with E2; the curation pass did not).
- **Small-Medium:** S3 Snapshots (Theme S — capture/restore/persist reuse existing machinery;
  the one net-new algorithm is a display diff).
- **Medium:** C3 clipboard over SSH/tmux (the terminal × tmux × SSH test matrix is the real cost) ·
  R1 editing-responsiveness (Theme R, in-brainstorm) · S1 rearrangeable outline / corkboard.
- **Larger:** S2 directory-as-binder (post-Effort-P plugin) · Theme P plugin candidates (all
  post-P — goals/streaks, style lens, CMS publish, backlinks, CriticMarkup/Fountain/wikilinks).
- **Noted (not scheduled):** H2 active_line eof-clamp.

Then **Effort P** (the in-process Lua plugin system) — the 1.0 capstone.
