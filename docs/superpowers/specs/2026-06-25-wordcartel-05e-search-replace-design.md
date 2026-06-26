# Wordcartel Effort 5e — Search & Replace — Design

**Date:** 2026-06-25
**Status:** Design (pre-plan)
**Effort:** 5e (after 5d focus-experience; before 5f Harper, 5g outline/folding)

## 1. Summary

Incremental in-document **search & replace** for Wordcartel. Literal-by-default
queries with an opt-in regex mode, a tri-state case toggle, live
jump-as-you-type with **all matches highlighted** (viewport-gated — the
roadmap §9.4 differentiator), forward/backward navigation with wrap-around,
and a live `N/M` match count. Replace ships in full (**C3**): one-shot
**replace-all** *and* interactive **query-replace** (`y`/`n`/`!`/`q`), with
`$1` capture references in regex mode.

The match engine is `regex-cursor` running directly over the `ropey` rope —
**search iteration is allocation-free** (no whole-document materialization).
Two bounded exceptions are explicit: replacement **capture-expansion** may
materialize the single matched region (§3.1), and the **oracle test**
materializes the rope by design (§9.1). Match-finding lives in a new IO-free
`wordcartel-core::search` module (oracle-tested against the `regex` crate on
materialized text); the overlay UI, key handling, viewport-gating, highlight
painting, and replace-commit live in the shell. No async runtime: search is
synchronous over the rope (full-document *background* search is explicitly out
of scope for v1).

## 2. Goals / Non-Goals

### Goals
- Literal-default query with `Alt+R` regex toggle; quiet handling of invalid regex.
- Tri-state case mode (`Alt+C`): **Smart** (default) → **Sensitive** → **Insensitive**.
- Incremental jump-as-you-type from a remembered origin; `Esc` cancels back to origin.
- Highlight **all** matches in the viewport; distinct style for the current match.
- `find-next` / `find-prev` with wrap-around + a wrapped indicator.
- Live `N/M` match count (full-document scan; cap deferred).
- **Replace-all** as a single undo unit, applied directly (no confirm — the
  single-undo-unit is the safety net, and query-replace covers per-match review).
- **Interactive query-replace**: per-match `y`/`n`/`!`/`q` stepping.
- `$1`..`$9` capture references in regex-mode replacements; literal text otherwise.
- Match-finding in `wordcartel-core` (IO-free, oracle-tested).

### Non-Goals (v1)
- Background/async full-document search (deferred; §10.3 of master design).
- Painting the **selection** (the highlight layer this effort adds *could*
  later paint selection, but selection-painting is out of scope here).
- Multi-line *literal* queries (the find field is single-line; regex `\n`/`(?s)`
  can still span lines — see §7).
- Search across multiple buffers / project-wide search (Effort 6+).
- Match-count caps for very large docs (deferred until measured — §6).
- Persisted search history / a searchable history ring.

## 3. Architecture

Functional-core / imperative-shell, matching the existing split.

```
wordcartel-core (IO-free, #![forbid(unsafe_code)])
  search.rs   NEW — compile a query → matcher; find_next / find_prev / count
                    over a RopeSlice via regex-cursor; pure, oracle-tested.

wordcartel (shell)
  search_overlay.rs NEW — Search overlay state machine (query/replace fields,
                          mode, case, direction, match cursor, origin, phase).
  render.rs         + byte-range highlight layer in the row builder
                    + search bar row (replaces status row while active).
  input.rs          + Ctrl+F / Ctrl+R / F3 / Shift+F3 key mapping (typed +
                      command-id paths).
  registry.rs       + search commands (find, replace, find_next, find_prev).
  editor.rs         + `search: Option<SearchState>` overlay field (XOR with
                      prompt/palette/menu/minibuffer).
  app.rs            + modal interception for the Search overlay in reduce()
                      + clear `search` on buffer swap / click-outside paths.
  commands.rs       + NEW multi-op replace helper. The existing edit helper
                      (range, replacement) is single-contiguous-range only; it
                      is the PATTERN, not a reused call. Replace-all needs a
                      hand-built multi-op ChangeSet (§4.5).
  editor.rs         + open_search; EVERY existing open_* must also clear the
                      new `search` field (the XOR set is not centralized — §3.3).
```

### 3.1 Core module: `wordcartel-core::search`

Pure functions over a rope. No editor, no overlay, no IO.

```rust
/// A compiled query plus the options it was built with.
pub struct Matcher { /* opaque: holds the compiled regex-automata Regex */ }

pub enum CaseMode { Smart, Sensitive, Insensitive }
pub enum QueryMode { Literal, Regex }

pub struct CompileError(pub String); // human-facing "invalid regex" message

/// Build a matcher. In Literal mode the needle is regex::escape-d.
/// CaseMode::Smart resolves to Insensitive unless `needle` has an uppercase
/// letter, computed here so the shell never re-implements the rule.
pub fn compile(needle: &str, q: QueryMode, case: CaseMode)
    -> Result<Matcher, CompileError>;

/// A half-open byte range [start, end) in rope coordinates.
pub struct Match { pub start: usize, pub end: usize }

/// First match with start >= `from`. None if no match at/after `from`.
pub fn find_next(rope: &Rope, m: &Matcher, from: usize) -> Option<Match>;

/// Last match with end <= `to`. None if no match at/before `to`.
pub fn find_prev(rope: &Rope, m: &Matcher, to: usize) -> Option<Match>;

/// All matches whose range intersects [lo, hi) — for viewport highlight.
pub fn matches_in(rope: &Rope, m: &Matcher, lo: usize, hi: usize) -> Vec<Match>;

/// Total match count over the whole rope (full scan). Non-overlapping.
pub fn count(rope: &Rope, m: &Matcher) -> usize;

/// Expand `$1`..`$9` / `${name}` in `template` against the captures of the
/// match at `at`. In Literal query mode the template is returned verbatim
/// (no capture expansion). Used by replace.
///
/// IMPLEMENTATION NOTE (Codex spec review): `regex-automata`'s interpolation
/// (`interpolate_string`) indexes capture spans into a CONTIGUOUS `&str`
/// haystack. Two viable builds, decided at the Task-1 spike (§9.3):
///   (a) re-run the engine in capture mode over the matched region only and
///       slice the (small, bounded) matched substring from the rope, then
///       interpolate against that; or
///   (b) compute capture spans via a cursor-captures path IF regex-cursor
///       surfaces one, slicing each group from the rope directly.
/// Either way only the MATCHED REGION is materialized, never the document.
/// If neither path is available for `regex-cursor`'s captures, literal-only
/// replacement (no `$N`) is the documented fallback.
pub fn expand_replacement(
    rope: &Rope, m: &Matcher, at: &Match, template: &str, mode: QueryMode,
) -> String;
```

The find/count functions iterate `regex-cursor`'s `RopeyCursor` over rope
chunks with **no document materialization**; only `expand_replacement` may
materialize the single matched region (above). Matches are **non-overlapping,
left-to-right** (standard `find_iter` semantics).

### 3.2 Shell overlay: `SearchState`

Lives in `Editor.search: Option<SearchState>`, mutually exclusive with the
other overlays (enforced the same way `open_prompt`/`open_palette` clear their
siblings).

```rust
pub struct SearchState {
    pub phase: Phase,            // Find | Replace | Stepping
    pub field: Field,           // which field has focus: Needle | Template
    pub needle: String,         // the find query (single line)
    pub template: String,       // the replacement (single line; empty in Find)
    pub cursor: usize,          // byte caret in the focused field
    pub mode: QueryMode,        // Literal (default) | Regex
    pub case: CaseMode,         // Smart (default) | Sensitive | Insensitive
    pub direction: Direction,   // last search direction (for n / N)
    pub origin: usize,          // caret position when search opened (Esc target)
    pub current: Option<Match>, // the match the caret is parked on, if any
    pub wrapped: bool,          // last navigation wrapped past an end
    pub error: Option<String>,  // "invalid regex" hint (regex mode only)
    pub buffer_id: BufferId,    // overlay is bound to the buffer it opened on
}

pub enum Phase { Find, Replace, Stepping }
pub enum Field { Needle, Template }
pub enum Direction { Forward, Backward }
```

`current` and the highlight set are **derived** each frame from `needle` +
options via the core API — `SearchState` stores no match list (one source of
truth, no staleness). The only cached match is `current`, recomputed on every
needle/option/navigation change.

### 3.3 Overlay XOR — the full site list (Codex spec review)

The XOR set is **not centralized** — each opener and close path clears its
siblings by hand. Adding `Editor.search` means touching every one of these
sites; the plan must enumerate them as a checklist, not assume a single point:

- `open_minibuffer`, `open_prompt`, `open_palette` (editor.rs ~237/255/268) —
  each must additionally set `self.search = None`.
- `open_search` (NEW) — must clear `prompt`, `minibuffer`, `palette`, `menu`,
  `pending_keys`, and `pending_mark` (mirror exactly what the existing openers
  clear — they clear `pending_keys`/`pending_mark`).
- The menu registry handler and the mouse click-outside path (registry.rs ~163;
  app.rs click-outside branch) — must close `search` like other overlays.
- Buffer swap / external swap — closes `search` (bound to `buffer_id`, §4.8).

**Reducer precedence.** The real `reduce()` order is `pending_mark` → menu →
palette → prompt → minibuffer → normal dispatch. Search is a bottom-line text
overlay like the minibuffer and is **mutually exclusive with it**, so it
intercepts at the minibuffer's level: insert `search` immediately after the
`minibuffer` branch and before normal dispatch. (Order between `search` and
`minibuffer` is immaterial since they can never be simultaneously `Some`.)

## 4. Data flow

### 4.1 Open
- `Ctrl+F` → `editor.open_search(Phase::Find)`: clears sibling overlays,
  records `origin = primary().head`, empties fields, `mode=Literal`,
  `case=Smart`, `phase=Find`, `field=Needle`.
- `Ctrl+R` → same but `Phase::Replace` (the bar shows both fields; focus starts
  in `Needle`).

### 4.2 Incremental find (per keystroke in `Needle`)
1. Edit `needle`/`cursor` (same codepoint-safe arithmetic as `Minibuffer`).
2. `search::compile(needle, mode, case)`:
   - `Err` (regex mode only) → set `error`, clear `current`, **don't move the
     caret**, highlight nothing.
   - `Ok(matcher)` → clear `error`; `current = find_next(rope, &matcher, origin)`
     (wrapping to `find_next(.., 0)` if none at/after origin). Move the buffer
     caret/selection to `current` (so the match is on-screen via the existing
     `ensure_visible`), but remember we can always return to `origin`.
3. Empty `needle` → `current=None`, caret stays at `origin`, no highlights.

### 4.3 Navigate
- `F3` / `Enter` → `find_next(rope, matcher, current.end)`; if `None`, wrap to
  `find_next(.., 0)` and set `wrapped=true`. `Shift+F3` / `Shift+Enter` →
  `find_prev(rope, matcher, current.start)` with symmetric wrap.
- After move, `ensure_visible` + `derive::rebuild` so the new match paints.

### 4.4 Highlight (render) — byte-range → visible-column via ColMap

**Key correctness point (Codex spec review):** matches are in **raw rope
bytes**, but a rendered row is the **concealed/styled view** — `StyledSeg`
(what rows are built from) carries no source range, and `VisualRow.src_span` is
only the row's min/max source span. So highlights **cannot** be painted from
row byte ranges directly. They must be projected through `ColMap.placed`, where
each `Placed{ src: Range<usize>, col, width, text }` maps a source byte range to
its visible column. A match spanning concealed markers (e.g. the `**` in
`**bold**`) correctly lands only on the visible glyphs whose `Placed.src`
intersects the match range.

Algorithm per frame (viewport-gated):
1. `let hits = search::matches_in(rope, &matcher, lo, hi)` for the visible byte
   span `[lo, hi)` (derived from the first/last visible logical lines, the same
   bounds the layout walk already computes).
2. For each visible row, walk its `ColMap.placed`; a glyph is highlighted iff
   its `Placed.src` overlaps any `hit`. Style = **current-match** (reversed) if
   the glyph's `src` overlaps `current`, else **other-match** (light, e.g.
   yellow bg). Concealed bytes have no `Placed` entry → silently skipped, which
   is the correct behavior.
3. In `SourcePlain`/`SourceHighlighted` render modes nothing is concealed, so
   `Placed.src` is the identity map and the projection is trivial; in
   `LivePreview` the projection is what makes highlights line up with concealed
   markdown.

This keeps highlighting O(visible glyphs) per frame. The `N/M` count uses
`count(rope, &matcher)` (full scan) plus the current match's ordinal, shown in
the search bar (§5).

### 4.5 Replace-all
> **Decision (whole-branch review, 2026-06-26):** replace-all applies
> **immediately on `Alt+A` with no `[y/n]` confirm**. Rationale: the whole
> operation is a single undo unit (`Ctrl+Z` reverts it in full), the status line
> reports the count afterward, and interactive query-replace (§4.6) already
> covers "review each match" — so a confirm prompt is redundant friction. (The
> earlier draft gated this behind `[y/n]`; that gate is intentionally removed.)

- `Replace All` (key in §5) on a non-empty valid needle:
  1. `count` → if 0, status "No matches" (no edit). If >0, proceed directly to step 2.
  2. Walk matches **left-to-right** and hand-build **one** multi-op
     `ChangeSet` over the **original** offsets — a sequence of
     `retain(gap) · delete(match) · insert(expanded-template)` ops covering the
     whole document (this is why no remapping is needed: nothing is applied
     until the full set is built). `ChangeSet` is a `Vec<Op>` applied
     sequentially, so it represents N disjoint replacements natively; this is a
     NEW shell builder, not the existing single-range `commands.rs` helper.
  3. `block_tree::Edit` is **single-range**, so pair the composed changeset with
     ONE conservative covering edit `Edit { range: first.start..last.end,
     new_len: <bytes from first.start..last.end after rewrite> }`. This
     over-widens the reparsed region but is always correct (block-tree's
     widen-or-full-fallback handles it) and keeps the contract simple.
  4. Apply as a **single** `editor.apply(txn, edit, EditKind::Other, clock)`.
     Because `editor.apply` is one `commit_coalescing` call producing one
     inverse changeset, this is intrinsically **one undo unit** — no separate
     history-grouping mechanism is required.
  5. Status: "Replaced N occurrences". The overlay closes and the caret lands at
     the **remapped `origin`** (`map_pos(origin, &cs)`) — Esc-to-origin semantics
     carried through the replacement, rather than at the last replacement.

### 4.6 Interactive query-replace (Stepping)
- `Replace` then step-key (§5) enters `Phase::Stepping`: park on the first match
  (`current`), highlight it as current, show `[y]es [n]o [!]all [q]uit  i/M`.
- `y` → apply this one replacement (single small `editor.apply`), advance
  `current` to the next match **in the post-edit rope** (remap — §4.7), repaint.
- `n` → advance `current` without editing.
- `!` → apply this and all remaining as one composed changeset (like §4.5 from
  `current` onward), exit stepping.
- `q` / `Esc` → stop; caret stays at `current` (where you quit). Closes overlay.
- When no next match remains → status "Replaced K occurrences", exit stepping.

### 4.7 Offset remapping (correctness)
- **Replace-all & `!`**: one composed `ChangeSet` over the *original* offsets;
  the **next match** needs no remapping because nothing is applied until the
  whole set is built.
- **Per-match `y` in stepping**: after each single `editor.apply`, the document
  shifts. Recompute the next match by calling `find_next(rope, matcher,
  parked_end_after_edit)` on the **mutated rope** — re-find rather than remap a
  stale match list. The net shift for the just-applied edit is
  `template_len - match_len`.
- **`origin` MUST be remapped (Codex spec review).** Re-finding fixes
  `current`, but the stored `SearchState.origin` (the Esc-cancel target, §4.8)
  is a byte offset into the *pre-edit* rope; any replacement at or before it
  shifts or deletes it. After **every** replacement commit (each `y`, and the
  composed replace-all / `!`), remap `origin` through that commit's `ChangeSet`
  via `change::map_pos` — exactly how `Buffer::apply` already remaps `marks`
  and `jump_ring` (editor.rs:91/94). This keeps Esc-to-origin valid after
  replacements.

### 4.8 Close / cancel
- `Esc` in Find/Replace → restore caret/selection to `origin`, drop the overlay.
- Confirming a navigation/replace and pressing `Esc` afterward still returns to
  `origin` (origin is the pre-search caret; this matches the documented filter
  Esc-cancel invariant).
- Switching the active buffer or any external buffer swap closes the overlay
  (it is bound to `buffer_id`).

## 5. UI / keys

The search bar **replaces the status row** while the overlay is active (same
real estate the minibuffer/prompt use), styled distinctly.

**Find bar:**
```
Find: teh␎              .* Aa~   3/17
      └ needle  └caret  │  │     └ current/total (or "no matches", "?" if invalid)
                        │  └ case indicator (Aa~ smart / Aa sensitive / aa insensitive)
                        └ regex indicator (.* when regex mode on; hidden in literal)
```

**Replace bar (two logical fields; Tab switches focus):**
```
Find:    (\w+), (\w+)        .* Aa   8 matches
Replace: $2 $1
```

**Stepping prompt:**
```
Replace "(\w+), (\w+)" → "$2 $1" ?   [y]es [n]o [!]all [q]uit   3/8
```

| Key | Context | Action |
|-----|---------|--------|
| `Ctrl+F` | editor | open Find |
| `Ctrl+R` | editor | open Replace |
| `F3` / `Enter` | Find/Replace | find-next (wrap) |
| `Shift+F3` / `Shift+Enter` | Find/Replace | find-prev (wrap) |
| `Alt+R` | overlay | toggle Literal ↔ Regex |
| `Alt+C` | overlay | cycle case Smart → Sensitive → Insensitive |
| `Tab` | Replace | switch focus Needle ↔ Template |
| `Alt+A` | Replace | Replace-All (applies immediately; status reports the count) |
| `Alt+Enter` | Replace | start interactive query-replace (Stepping) |
| `y`/`n`/`!`/`q` | Stepping | replace / skip / rest / quit |
| printable | Find/Replace | insert into focused field |
| `Left`/`Right`/`Backspace` | Find/Replace | edit focused field (codepoint-safe) |
| `Esc` | any | cancel → restore origin caret, close overlay |

> **Key-collision check (Codex spec review — bindings are config-driven via
> `keymap.rs` presets, not just `input.rs`):**
> - In the **CUA default** preset, `Ctrl+F`, `Ctrl+R`, `F3`, `Shift+F3`,
>   `Alt+*`, `Tab` are unbound (`Ctrl+E`=filter, `Ctrl+T`=transform,
>   `Ctrl+Y`/`Ctrl+Shift+Z`=redo are the only nearby binds). Redo is **not**
>   `Ctrl+R`, so `Ctrl+R`=replace is clean in CUA.
> - The **WordStar** preset already binds `ctrl-f → move_right`
>   (keymap.rs:298). A preset that rebinds a search key **shadows** the search
>   command — expected preset behavior, not a bug. The contract is: register the
>   search **commands** (`find`/`replace`/`find_next`/`find_prev`) in the
>   registry and bind them in the **CUA default**; preset/user keymap patches
>   may override per their own rules.
> - **Terminal-delivery caveat:** `Alt+letter`, `Alt+Enter`, and `Shift+F3` are
>   representable in the chord parser/trie, but reliable delivery depends on the
>   terminal. Treat each as an explicit **test/verification item** in the plan;
>   provide a non-`Alt` fallback path for the regex/case toggles if a target
>   terminal drops them (e.g. a toggle command reachable from the palette).

## 6. Performance / responsiveness

Responsiveness is the project's top priority (no silent UI waits).

- **Highlight** is viewport-gated: `matches_in` scans only the visible byte span
  → O(visible rows), independent of doc size. Safe per-frame.
- **`count`** is a full-document scan. `regex-automata` runs on the order of
  GB/s, so a full count is sub-millisecond to ~1–2 MB (the same budget the
  block-tree spike targets). v1 does the full scan every time the needle/options
  change. **Cap deferred**: if profiling on multi-MB docs shows stutter, add a
  bounded count (`3/300+`) — explicitly out of scope now, noted so a future
  effort doesn't think it was missed.
- **Incremental find** computes one `find_next` per keystroke from `origin` —
  O(distance to next match), not O(doc).
- No new thread, channel, or async; the existing `Msg`/`reduce` loop is
  untouched except for one new overlay branch.

## 7. Regex / matching semantics

- **Whole-rope stream.** `regex-cursor` matches over the entire rope, so a regex
  *can* span lines (`foo\nbar`, `(?s).`). The **find field is single-line**, so
  the user can't *type* a literal newline; multi-line matching is a regex-mode
  capability via escapes, not a literal-query feature.
- **`.` excludes `\n`** by default (standard); `(?s)` opts in.
- **Non-overlapping, left-to-right** match iteration (standard `find_iter`).
- **Smart-case** is resolved in `search::compile`: scan the *needle* for an
  uppercase letter; if present → Sensitive, else → Insensitive. Computed once at
  compile, so navigation/highlight never re-derive it.
- **Empty needle** → no matcher, no matches, no caret movement.
- **Capture refs** (`$1`..`$9`, `${name}`) expand only in Regex mode; in Literal
  mode the replacement template is inserted verbatim (a literal `$1` stays `$1`).

## 8. Error handling

| Situation | Behavior |
|-----------|----------|
| Invalid regex (regex mode, mid-type) | `error` set, bar shows `?` where count goes, no highlight, **caret unmoved** |
| Invalid regex on navigate/replace | no-op + status "invalid regex"; never mutate |
| Empty needle | no matches, no movement, replace keys no-op |
| Replace-all, 0 matches | status "No matches", no confirm, no edit |
| `$N` ref out of range | expands to empty string (regex-automata semantics) |
| Buffer swapped while overlay open | overlay closes (bound to `buffer_id`) |
| Needle matches empty string (e.g. `a*`) | `find_next` advances to the **next UTF-8 char boundary** after a zero-width match to guarantee progress (≥1 byte but never mid-codepoint — rope slicing requires char boundaries); replace-all on a zero-width match is a no-op for that position |

## 9. Testing

### 9.1 Core (`wordcartel-core::search`) — oracle-tested
- **Oracle:** for randomized (rope, pattern, options) triples, assert
  `find_next`/`matches_in`/`count` over the rope **equal** the plain `regex`
  crate run on `rope.to_string()` (the block-tree oracle pattern). Covers
  literal-escape, smart-case resolution, multi-byte UTF-8 boundaries, matches at
  rope-chunk boundaries (the regex-cursor failure mode), zero-width matches.
- **Unit:** `compile` literal-escapes metacharacters; smart-case uppercase
  detection; `expand_replacement` capture expansion + literal passthrough +
  out-of-range `$N`; `find_prev` symmetry; wrap boundary (`find_next` past end).

### 9.2 Shell
- `SearchState` transitions: open records origin; Esc restores origin; needle
  edit recomputes `current`; tri-state case cycle; mode toggle re-escapes.
- Navigation wrap sets `wrapped`; direction tracked.
- Replace-all builds **one** undo unit (undo reverts all N; redo re-applies all).
- Query-replace stepping: `y` edits + advances on the mutated rope; `n` skips;
  `!` finishes remainder as one unit; `q`/`Esc` leaves caret at quit point.
- Offset correctness: `y` on a match whose replacement changes length still lands
  the next `current` on the right text (re-find on mutated rope).
- Overlay XOR: opening search clears prompt/palette/menu/minibuffer and vice
  versa; buffer swap closes search.
- Render: a `matches_in` set styles the right glyphs **via `ColMap.placed`
  projection**; current match distinct; highlight is viewport-gated (off-screen
  matches not painted); a match spanning concealed markdown (`**bold**` in
  LivePreview) highlights only the visible glyphs (the `**` markers, having no
  `Placed` entry, are skipped); search bar composition
  (needle/caret/indicators/count) within width.
- Overlay XOR completeness: assert each `open_*` clears `search` and
  `open_search` clears all siblings + `pending_keys`/`pending_mark` (§3.3).
- Key mapping: `Ctrl+F`/`Ctrl+R`/`F3`/`Shift+F3` map to the right commands and
  don't collide (assert against the live keymap).

### 9.3 Dependency build gate (early task)
- Adding `regex-cursor` + `regex-automata` to `wordcartel-core` must:
  (a) compile against the **pinned** `ropey = "=1.6.1"` (confirm `RopeyCursor`
      consumes a 1.6.1 `RopeSlice` / its chunk API),
  (b) not violate core's `#![forbid(unsafe_code)]` (the deps may use unsafe
      internally; our code may not),
  (c) license MIT/Apache (per §9.1 of the master design),
  (d) settle the **captures path** for `expand_replacement` (§3.1): confirm
      whether `regex-cursor` exposes a rope-cursor captures API, or whether we
      materialize the matched region and use `regex-automata`'s
      `interpolate_string`. If neither yields `$N`, record literal-only
      replacement as the shipped fallback and note it in the plan.
  This is **Task 1** of the plan — prove the build AND the captures path before
  any feature code, so a dependency surprise (the roadmap §10.3 flags
  regex-cursor's relative immaturity) can't sink the effort mid-stream.

## 10. Dependencies

- **New in `wordcartel-core`:** `regex-cursor`, `regex-automata`. Both MIT/Apache.
- No new shell dependencies.
- No change to the pinned `ropey = "=1.6.1"`.

## 11. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| `regex-cursor` immaturity / ropey-pin incompatibility (§10.3) | Task 1 build gate before feature code; oracle test catches chunk-boundary bugs |
| Full-scan `count` stutter on multi-MB docs | viewport highlight is already gated; count cap is a noted, scoped follow-up |
| Offset shift during query-replace corrupts later matches | replace-all/`!` use one composed changeset; per-match `y` re-finds on the mutated rope |
| Highlight layer is the first byte-range styling in the row builder | isolated, additive change; default-empty highlight set is a true no-op for existing render tests |
| Zero-width regex (`a*`) infinite loop | `find_next` advances to the next UTF-8 char boundary after a zero-width hit (≥1 byte, boundary-safe) |
| Match highlight misaligns with concealed markdown | highlights projected through `ColMap.placed[].src` (§4.4), not raw row byte ranges |
| `origin` Esc-target invalidated by replacements | remap `origin` through each commit's `ChangeSet` via `map_pos` (§4.7), as marks/jump-ring already do |
| `regex-cursor` lacks a usable rope-captures path for `$N` | Task-1 spike decides materialize-matched-region vs cursor-captures; literal-only replacement is the documented fallback (§3.1) |
| Preset rebinds a search key (e.g. WordStar `ctrl-f`) | expected — commands bound in CUA default; presets/user patches override per their rules (§5) |
| Overlay key handling competes with normal editing | Search is a modal overlay in the XOR set; reduce() intercepts before the normal keymap (same as prompt/palette/menu) |

## 12. Out of scope → future efforts

- Background/async full-document search (workers; §10.3).
- Selection painting (the highlight layer makes it cheap later).
- Project-wide / multi-buffer search (Effort 6+).
- Search-history ring; match-count cap for huge docs.
