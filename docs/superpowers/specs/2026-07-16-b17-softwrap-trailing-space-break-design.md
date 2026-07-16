# B17 — Soft-wrap trailing-space-at-margin caret feedback (design note)

**Status:** design (Codex-gated). **Effort:** B17. **Date:** 2026-07-16.
**Crate surface:** `wordcartel-core/src/layout.rs` (core), with verify-don't-regress sweeps in
`wordcartel/src/render.rs`, `nav.rs`, `ventilate.rs`, `derive.rs`.
**Amends:** the "spec D2" hang rule (see [History](#history) — this note is the deliberate recorded
amendment).

---

## 1. Problem & approved target

Typing a space at the soft-wrap margin gives **no caret feedback**. The mechanism is two cooperating
pieces in the real code:

1. **The hang rule** — `layout.rs:322` `is_ws`, `:325` `hang = is_ws && !CodeBlock`, `:332`
   `while !hang && col.saturating_add(vg.width) > vw && col > prefix_width`. A trailing space is
   `hang == true`, so the overflow `while` is skipped; the space is placed *past the edge on the same
   visual row* (`layout.rs:374` unconditional push, `:375` `col = col.saturating_add(vg.width)`), and
   `row_end_col[last]` becomes `vw + 1` (`:377`). The line does not gain a row; the break defers to the
   next non-ws grapheme. Pinned today by `word_wrap_trailing_whitespace_hangs_past_edge`
   (`layout.rs:658`).
2. **The caret clamp** — `render.rs:452-457`: the caret, now logically at the hung column (`> text_width`),
   is clamped `col.min((tg.text_width as usize).saturating_sub(1))` (`render.rs:456`) back onto the last
   content cell, so it shows **no advance**. `ColMap::source_to_visual` (`layout.rs:86-101`) is what
   returns the hung column via its `offset >= self.eol` branch (`:87-90`).

The space *is* written to the buffer (data-safe), but reads as "the spacebar didn't register," so
writers press space again → a stray double-space. Appears in the plain buffer and every lens (shared
engine).

**Approved target (2026-07-16, all five forks resolved on the "phantom flush row" slate):** a trailing
space at/past the margin **breaks** — the caret moves to **column 0 of the next visual row**, the
continuation is **flush at the left margin with NO leading-space indent**, the break-space is consumed
at the boundary, and the next typed word lands at col 0. This **amends spec D2**: whitespace at the
margin no longer hangs unboundedly — it hangs *and* opens an empty flush continuation row that the caret
occupies.

### Resolved forks (governing decisions — do not relitigate)
- **(a) Phantom flush row.** The break-space **stays hung at the end of row N**; layout appends **one
  empty flush visual row N+1**. (Rejected: migrating/suppressing the space onto N+1; dropping it.)
- **(b) Multi-space run.** The whole trailing run hangs on row N; the caret **parks at (N+1, col 0)**
  for the entire run. Extra spaces do not advance it; backspace does not move it until the *last* space
  deletes, popping the caret to the pinned hang edge.
- **(c) Uniform across render modes + EOF.** The phantom row appears wherever a line's visible text
  reaches the margin, in **all** render modes and at EOF. The human explicitly accepts the rare visible
  blank row in a concealed/hard-break static line that reaches the margin. **No caret-line-only gating.**
- **(d) Include exact-fill.** The rule triggers when the final row ends in a whitespace run whose **end
  column ≥ vw** (so a space landing exactly at the last cell, end col `== vw`, also triggers).
- **(e) Ventilate propagation.** Let it propagate through `ventilate::layout_block` — **but see §7:
  the verification shows it is MOOT**, because ventilated sentence displays can never end in trailing
  whitespace.

---

## 2. The rule and its placement

Insert a **post-loop epilogue** in `layout()` (`layout.rs:244`), between the final
`row_end_col.push(col)` (`layout.rs:377`) and `let rows = row + 1;` (`layout.rs:378`):

```
// B17: a trailing whitespace run hung AT/PAST the margin gets an empty flush continuation
// row, so the caret after the space lands at (next_row, prefix_width) = col 0 of the text
// column — feedback that the space registered. Non-CodeBlock only (code preserves byte-
// identical wrap; the hang rule is already off for it, layout.rs:325). Amends spec D2.
if !matches!(role, BlockRole::CodeBlock)
    && col >= vw
    && placed.last().is_some_and(|p| p.text == " " || p.text == "\t")
{
    row += 1;
    row_end_col.push(prefix_width);
}
```

Grounding of every term:
- `col` at this point **is** `row_end_col[final]` (just pushed at `:377`); it is the end column of the
  final row after the last grapheme, computed via `col.saturating_add(vg.width)` (`:375`).
- `vw` is the clamped viewport width `viewport_width.max(1)` (`layout.rs:252`) — the same bound the hang
  `while` compares against (`:332`), so "reached the margin" is defined identically.
- `placed.last()` is the last placed grapheme (`Placed`, `layout.rs:13`); the `is_ws` test mirrors
  `layout.rs:322` (`" "` or `"\t"`). A hung *over-wide grapheme* (e.g. `中` at vw 1) fails this guard →
  no phantom row (out of scope: over-wide graphemes already give visible feedback).
- `prefix_width` (`layout.rs:285-302`, `ColMap.prefix_width` `:81`) is the hanging-indent column — 0 for
  a plain paragraph, 2 for a `• ` list bullet, `GUTTER_COLS` (6) under ventilate. Pushing it as the
  phantom row's end column makes the caret land flush under the text column, consistent with the
  existing continuation-row hanging indent (`prefix_reduces_wrap_capacity`, `layout.rs:880`).

The empty row itself is materialized for free: `visual_rows` is built as
`vec![VisualRow { display: String::new(), .. }; rows]` (`layout.rs:380-381`) with `rows = row + 1` now
including the phantom, so an empty, well-formed `VisualRow` (empty display/segs, width 0, role
propagated at `:417-419`) already exists — the same shape a blank logical line produces today.

### What does NOT trigger (boundary of the rule)
- **Exact-full-of-letters** (`"abcd"` @ vw 4): last placed is `d`, not ws → **no phantom row**.
  Explicitly OUT of scope (§6) — letters give next-keystroke feedback; only spaces are invisible.
- **A trailing space well within the margin** (`"ab "` @ vw 8): `col == 3 < vw == 8` → no phantom row.
- **A line ending in a word after a hung space** (`"- aaaa bbbb"` @ vw 6, `layout.rs:880`): the final
  row ends in `bbbb`, last placed is `b` → no phantom row; that test is unaffected.
- **CodeBlock**: guarded off (byte-identical wrap preserved; `word_wrap_codeblock_space_wraps_not_hangs`
  `layout.rs:711`).
- **Empty line** (`""`): `placed` is empty → `placed.last()` is `None` → no phantom row (stays 1 empty
  row).

---

## 3. Why the caret behavior falls out of EXISTING code (no resolver change)

The phantom row needs **zero changes** to `ColMap::source_to_visual` / `visual_to_source`:

- **Caret after the space → (phantom_row, col 0).** For `"abcd "`, `eol = line.len() = 5`
  (`ColMap.eol` `layout.rs:70`, set at `:425`). The caret sits at byte 5 = eol. `source_to_visual(5)`
  takes the `offset >= self.eol` branch (`layout.rs:87-90`): `row = rows.saturating_sub(1)` = the phantom
  row, returns `(phantom_row, row_end_col[phantom])` = `(phantom_row, prefix_width)`. For a plain
  paragraph that is `(1, 0)` — exactly col 0 of the next visual row. **No edit to the resolver.**
- **Next word lands there.** Typing at byte 5 inserts, re-layout yields `"abcd x"`, whose real wrap
  already places `x` at `(1, 0)` (break arm `Some(b) if b == i`, `layout.rs:343`, with the space hung on
  row 0). The phantom row renders the *transient* trailing-space state as the geometry the next word will
  actually occupy — no jump when the word arrives.
- **Click / vertical-overshoot on the phantom row.** `visual_to_source(phantom_row, any_col)` finds no
  placed grapheme on that row (all placed are on row ≤ N), so it takes the **empty-row branch**
  (`layout.rs:148-155`): it falls forward to the next row's first grapheme, else `self.eol`. For the
  phantom (last) row it returns `eol` — a valid stop. `end_of_row_clamps_not_teleports` (`layout.rs:781`)
  is untouched (it tests a *non-empty* short row).
- **The render clamp becomes inert for this case but STAYS.** `render.rs:456`
  `col.min(text_width - 1)`: the phantom-row caret column is `prefix_width` (0 for a paragraph) which is
  `< text_width`, so the clamp is a no-op here. **It must remain** — it still guards (i) the
  hang-*interior* caret (§4) and (ii) a genuinely over-wide single grapheme past the rect. **Action:**
  update its comment (`render.rs:453-454`, which currently narrates only the D2 hang-clamp) to note that
  the trailing-space case now resolves to the phantom row and the clamp's residual duty is the
  hang-interior / over-wide-grapheme guard.

---

## 4. Invariants & boundary behaviors to PIN

These are behavior changes or subtle non-changes that tests must lock (grounded, then listed as tests in
§5):

- **Hang-interior caret stays pinned.** The caret *before* the space (byte 4 of `"abcd "`) is `4 < eol`;
  `source_to_visual(4)` matches the space (`src 4..5`, `src.start >= 4`) → `(0, 4)` (`layout.rs:91-93`),
  which the render clamp pins to `(0, 3)`. So **Left from `(1,0)`** (byte 5 → byte 4) shows the caret at
  the pinned `(0, 3)`, and **backspacing the last space** (byte 5 delete → `"abcd"`, caret byte 4 = eol,
  `source_to_visual(4)` → `(0, 4)` → pinned `(0, 3)`) pops the caret to the same pinned hang edge. This
  is inherent to keeping the space hung (fork a) and must be pinned so it is not later reported as a bug.
- **End / Home semantics shift by the phantom row.** `move_end` on row 0 of `"abcd "` (`layout.rs:518`)
  reads `row_end_col[0]` = 5, `visual_to_source(0, 5)` → clamps to the row-0 end = eol (byte 5), snaps to
  a stop = 5. But `screen_pos` (`nav.rs:107`) resolves the *visible* caret from the offset alone
  (`source_to_visual(snapped)` — it does not consult `layout::Cursor.row` affinity), so the visible caret
  for that offset is `(phantom_row, 0)`. Defensible (eol genuinely lives on the phantom row now), but
  End/Home tests must pin it. `enter_from_bottom` into such a line lands on the phantom row at eol for
  that one step regardless of desired_col.
- **B10 is a NON-collision (but pin it jointly).** The phantom is a *visual* row inside line L's own
  ColMap; B10's phantom is a *logical* line handled by `nav::caret_line` (`nav.rs:45-54`, `:53`
  `byte_to_line(h.min(buf.len()))`). For `"abcd \n"` with the caret at EOF, both exist, stacked: the
  blank phantom visual row of line 0 above the B10 EOF caret row. No code coordination is needed, but a
  joint pin test guards the stack from future regressions.
- **The adjacent symptom is OUT of scope.** A row exactly full of *letters* has the identical
  frozen-caret feel and is **not** fixed by B17 (§6). State it so B17 is not called half-done.

---

## 5. The Law-green claim (to be CONFIRMED by `cargo test`)

**Claim:** all six wrap property laws (`layout.rs mod props`, `:952-1376`, 512 cases each) pass
**UNMODIFIED** under the phantom-flush-row shape. Rationale, per law, grounded on the fact that the space
**stays placed exactly where it is today** and only an empty row is appended:

- **Law 4 / Law 3(a) reconstruction** (`layout.rs:1101-1103`, and `law4_active_identity` `:1183`): the
  `placed` vector is byte-identical to today (the epilogue pushes no `Placed`), so
  `placed.iter().map(|p| p.text).collect()` still reconstructs the visible content. The space is never
  dropped.
- **Law 3(c) width bound** (`layout.rs:1114-1137`): row N is unchanged (the trailing-ws subtraction at
  `:1126-1131` still applies); the phantom row has no placed graphemes, so its `sum == row.width == 0`
  and `content_width == 0 <= w`. Green.
- **Law 3(d) contiguity** (`layout.rs:1139-1144`): `max_row` is computed over `placed` (= final *content*
  row N, since the phantom has no placed), and the assert is
  `max_row + 1 == map.rows.min(max_row + 1)`. The `.min(max_row + 1)` clamps `map.rows` (= N+2) down to
  N+1, so `N+1 == N+1`. **This `.min` is precisely what tolerates a trailing empty row** — the load-
  bearing subtlety Codex should verify.
- **Law 1 round-trip** (`layout.rs:1016-1034`): iterates `map.placed` only; every placed cell is
  unchanged. Green.
- **Law W1 no-needless-break** (`layout.rs:1157-1176`): for the phantom row `r`,
  `map.placed.iter().position(|p| p.row == r)` is `None` → the `let-else` `continue`s (`:1169`). Green.
- **Law 2 / Law 6 nav** (`layout.rs:1041`, `:1244`): both probe every row including the phantom via
  `row_end_col` and overshoot columns. `visual_to_source(phantom, col)` hits the empty-row branch
  (`:148-155`) → `eol`, a valid stop; `move_end`/`move_home`/`enter_*` on the phantom row all land on
  `eol`. Green.
- **Law 5 desired-col** (`layout.rs:1211`): down onto the phantom row → `eol`; up →
  `visual_to_source(0, desired_col)` returns the row-0 start offset for that desired col. Round-trips.

All hand-derivations survive because the hang placement survives; **the implementer MUST confirm with
`cargo test -p wordcartel-core` before claiming green** (per CLAUDE.md: `cargo` is ground truth for
changed code). If any law reddens, it signals the epilogue perturbed `placed` — which it must not.

---

## 6. Scope

**IN:**
- The `layout()` epilogue + trigger condition (§2): non-CodeBlock, final-row trailing-ws run, end col
  ≥ vw → append one empty flush row at `prefix_width`.
- Rewrite of `word_wrap_trailing_whitespace_hangs_past_edge` (`layout.rs:658`) and the render golden
  `hung_trailing_space_caret_pins_at_edge` (`render.rs:2005`).
- New coverage (§5 test plan).
- Comment update at `render.rs:453-454` (the D2 clamp comment) and the `layout.rs:304/323/326` spec-D2
  comments to name the amendment.

**OUT:**
- The exact-full-of-**letters** row caret pin (adjacent symptom; a separate item if ever wanted).
- Any `sentence_spans` / `segment_block` / ventilate segmentation redesign (§7 is *verify-only*).
- B10 itself (already shipped — verify-don't-regress).
- UAX #14 break-mapping changes (`visible_break_indices`, `layout.rs:209`).

**command-surface-contract:** **N/A** — B17 touches no commands, user-settable options, palette, menu,
or keybinding hints. It changes only pure layout geometry inside `wordcartel-core`. (Stated explicitly
per the contract's conformance clause.)

**Module budgets:** the core change is ~6 lines inside the existing
`#[allow(clippy::too_many_lines)]` `layout()` (`layout.rs:243`); no new hub bulk. `render.rs` is only a
comment edit (`module_budgets.rs:59` cap @ 900 unaffected).

---

## 7. (e) Ventilate verification — the propagation is MOOT

**Finding: a ventilated block can never gain a mid-block or end-of-block phantom row, because a
ventilated sentence's display string cannot end in trailing whitespace.**

Chain, grounded:
- `layout_block` (`ventilate.rs:288-327`) lays out each sentence via
  `layout::layout(&display, .., GUTTER_COLS)` (`:301-302`), where
  `display = sentence_display(&raw[sf..st])` (`:299`) and `(sf, st)` come from `segment_block`
  (`:297`), a thin re-export of `sentence_spans` (`ventilate.rs:73-74`, `:13`).
- `sentence_spans` produces spans with **no trailing whitespace** — documented invariant
  (`textobj.rs:190-192` "Each span is `(from, to)` byte offsets with **no trailing whitespace** (§3)")
  and enforced by `content_end` = `trim_end_matches(char::is_whitespace)` (`textobj.rs:69-72`). The S5
  test `textobj.rs:303-304` pins that a trailing space after a sentence is dropped from the span.
- `sentence_display` only maps **interior** `\n` → space (`ventilate.rs:57-58`
  `raw_span.replace('\n', " ")`); it does not append whitespace and cannot introduce a trailing space
  because the span already ends at a content byte.

Therefore no `display` handed to `layout()` under the lens ends in a space/tab → the §2 trigger
(`placed.last()` is ws) is never satisfied on a ventilated sentence → **no phantom row appears under the
ventilate lens**, mid-block or at block end. The stitching math (`row_base`, `row_end_col`, gutter
`Count`/`Continuation` at `ventilate.rs:303-318`) is consequently unchanged, and the extra
`Continuation` gutter cell I speculated about in the scope memo does not materialize.

**Consequence for the test plan:** the ventilate check is a **negative** assertion — a paragraph whose
sentence reaches the margin gains **no** phantom row under the lens (guarding against a future change to
`sentence_spans` that stops trimming). Fork (e) is thus effectively moot; propagation is a non-event, not
a feature.

---

## 8. Test plan

### Rewrites (2)
1. **`word_wrap_trailing_whitespace_hangs_past_edge`** (`layout.rs:658`). New expectations for
   `"abcd "` @ vw 4: `rows.len() == 2`; `map.row_end_col[0] == 5` (space still hung on row 0, Law 4);
   `map.placed.len() == 5` (space still placed); `map.rows == 2`; `map.source_to_visual(5) == (1, 0)`
   (caret after the space at col 0 of the phantom row); `map.row_end_col[1] == 0` (phantom flush at
   prefix_width). Rename to reflect "hangs-then-breaks".
2. **`hung_trailing_space_caret_pins_at_edge`** (`render.rs:2005`). New expectations for `"abcd \n"`
   caret at byte 5: the render-captured hardware caret is at `x == 0` of the **next** screen row
   (`y == +1`), not pinned at `x == 3`. Rename accordingly.

### New unit tests (`layout.rs mod tests`)
3. **Multi-space run** (fork b): `"abcd   "` @ vw 4 → `rows == 2`; all three spaces placed on row 0
   (`row_end_col[0] == 7`, `placed.len() == 7`); `source_to_visual(eol) == (1, 0)`; the phantom row is
   singular (not one per space).
4. **Exact-fill** (fork d): `"abc "` @ vw 4 → end col `== vw == 4` triggers; `rows == 2`,
   `source_to_visual(eol) == (1, 0)`.
5. **Trailing tab**: `"abc\t"` @ vw 4 (tab width 4, `TAB_WIDTH` `layout.rs:9`) → hung tab triggers a
   phantom row; caret at `(1, 0)`.
6. **List-prefix hanging indent**: `"- aaaa "` @ vw 6 (prefix_width 2) → `rows == 2`,
   `row_end_col[phantom] == 2`, `source_to_visual(eol) == (1, 2)` (flush under the text column, not col
   0 of the screen).
7. **CodeBlock unchanged** (epilogue does not fire for CodeBlock — behavior byte-identical to the
   pre-B17 greedy wrap): `"abcd "` as `BlockRole::CodeBlock` @ vw 4. CodeBlock disables the hang
   (`layout.rs:325` `hang = is_ws && !matches!(role, CodeBlock)`) and has no break opportunities
   (`Vec::new()`, `layout.rs:304-307`), so the trailing space **already wraps to row 1 today** via the
   grapheme fallback (`:365-370`, `:374-375`): `rows == 2`, `row_end_col == [4, 1]`, the space placed at
   `(1, 0)`. B17's `!matches!(role, CodeBlock)` epilogue guard means it adds **no extra row** on top of
   that. Assert the full pre-B17 shape — `rows == 2`, space at `(1, 0)`, `row_end_col[0] == 4`,
   `row_end_col[1] == 1` — and that it is identical with and without B17 (no phantom row appended). The
   sibling `word_wrap_codeblock_space_wraps_not_hangs` (`layout.rs:706-713`, `"abcd x"` → row0 `"abcd"`,
   row1 `" x"`) also stays green.
8. **Whitespace-only line** (uniform rule): `"    "` @ vw 4 → the run reaches the margin → one phantom
   row; caret at `(1, 0)`. Pin whatever the uniform rule yields (accepted per fork c).
9. **Non-margin trailing space negative**: `"ab "` @ vw 8 → no phantom row (`rows == 1`).
10. **Phantom-row nav/click**: on `"abcd "` @ vw 4 — `move_home`/`move_end` on the phantom row land on
    valid stops (eol); `visual_to_source(1, 9)` (overshoot) → eol; `move_up_within` from the phantom row
    returns to row 0 at the desired column; a click on `(1, 0)` round-trips to eol.
11. **Hang-interior pin** (§4): caret at byte 4 of `"abcd "` → `source_to_visual(4) == (0, 4)` (pins to
    the hung cell, later clamped by render); `move_left` from eol lands on byte 4.

### New shell tests
12. **B10 joint pin** (`render.rs` or `nav.rs`): `"abcd \n"` with the caret at EOF renders the blank
    phantom visual row of line 0 *above* the B10 EOF caret row; both are present and the caret is placed
    on the EOF logical line, not the phantom visual row.
13. **Ventilate negative** (`ventilate.rs` unit or `e2e.rs`): a ventilated paragraph whose sentence
    reaches the wrap margin gains **no** phantom row under the lens (§7); block row count equals the
    pre-B17 count. (Guards the `sentence_spans` trim invariant.)
14. **Property re-run**: confirm Laws 1–6 (`layout.rs:1016-1374`) stay green unmodified (§5) — this is a
    `cargo test -p wordcartel-core` gate, not new code.

### Regression guard
Before implementation, `grep` for any test asserting an exact `rows.len()` / `map.rows` on a line ending
in a hung margin space. Audit done for this note: the only such assertions are `layout.rs:661` (the test
being rewritten) and `layout.rs:744` (`concealed_bold_drops_markers_in_display`, `"**bold**"` — no
trailing space, unaffected). The two B1 goldens (`render.rs:1945`, `:2044`) and the ventilate e2e end in
words/content-only sentences → no phantom row → unaffected.

---

## History

- **2026-07-16 — spec D2 amended (this note).** "Spec D2" has **no standalone spec document** in the
  repo; it originated in the layout spike (`~/projects/wordcartel-layout-spike`, per the file header
  `layout.rs:1-2`) and lives ONLY as code comments: `layout.rs:304` ("UAX #14; spec D1/D2"), `:323`,
  `:326`, `:350`, `:713` ("spec D2 as amended"), and `render.rs:454` ("spec D2 clamp"). Prior triage
  prose is in `docs/ux-backlog.md` (B17 section, marker `<!-- item: … -->`). The prose "D2" in
  `docs/design/prose-structure-arc.md:61` and elsewhere is an **unrelated** sentence-authority decision
  — not this hang rule.

  **The amendment:** whitespace at the soft-wrap margin no longer hangs as a terminal state. It still
  hangs (stays placed on row N, preserving Law 4 and the byte-safe insert path), but `layout()` now
  appends one **empty flush continuation row** (col 0 / `prefix_width`) that the post-space caret
  occupies, giving the writer real line-move feedback. Applies to all non-CodeBlock roles and all render
  modes (fork c). Because there is no spec-doc D-section, the amendment is recorded HERE (design note
  History) and reflected in the code comments at the sites above during implementation (the deliberate
  recorded act per CLAUDE.md's command-surface / contract-amendment discipline, applied by analogy to
  the layout spec that lives in comments).
